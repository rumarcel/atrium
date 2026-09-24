//! The pairing service against a real database, without the network: the
//! ADR-019 sealing guarantees, restarts, concurrency and the transaction
//! boundaries. The connection a request "arrived on" is a value here, so
//! these tests can name it; the network suite (`http::pairing_tests`) shows
//! the same properties over real TLS.

use std::sync::{Arc, Mutex};

use time::OffsetDateTime;

use atrium_pairing::client::ClientPairing;
use atrium_pairing::device::{DeviceMetadata, DeviceName, Platform};
use atrium_pairing::secret::PairingSecret;
use atrium_pairing::wire::{BeginResponse, CompleteRequest, CompleteResponse};

use super::{arm, Arrival, Clock, Pairing, Refused, ServerFacts, ATTEMPT_WINDOW, MAX_ATTEMPTS};
use crate::db::{self, Database};
use crate::http::connection::ConnectionId;
use crate::http::limits::SourceKey;
use crate::identity::{SecretsKey, ServerId, SpkiPin};
use crate::testutil::Fixture;

fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).expect("random");
    bytes
}

/// A connection, as the service sees one.
struct Conn {
    id: ConnectionId,
    source: SourceKey,
    exporter: [u8; 32],
}

impl Conn {
    fn new(source: u32) -> Self {
        Self {
            id: ConnectionId::generate().expect("id"),
            source: SourceKey::V4(source),
            exporter: random(),
        }
    }

    fn arrival(&self) -> Arrival<'_> {
        Arrival {
            connection: self.id,
            source: self.source,
            exporter: Some(&self.exporter),
        }
    }
}

struct Harness {
    fixture: Fixture,
    secrets: Arc<SecretsKey>,
    now: Arc<Mutex<OffsetDateTime>>,
    facts: ServerFacts,
    pairing: Pairing,
}

impl Harness {
    fn new() -> Self {
        let fixture = Fixture::new("pairing");
        drop(db::create(&fixture.layout).expect("database"));
        let secrets = Arc::new(SecretsKey::from_bytes(random()));
        let now = Arc::new(Mutex::new(
            OffsetDateTime::from_unix_timestamp(1_900_000_000).expect("time"),
        ));
        let facts = ServerFacts {
            server_id: ServerId::parse(&hex::encode(random::<16>())).expect("id"),
            spki: SpkiPin::of(&random::<64>()),
            name: "unit".to_owned(),
        };
        let pairing = Self::service(&fixture, &secrets, &now, &facts);
        Self {
            fixture,
            secrets,
            now,
            facts,
            pairing,
        }
    }

    fn service(
        fixture: &Fixture,
        secrets: &Arc<SecretsKey>,
        now: &Arc<Mutex<OffsetDateTime>>,
        facts: &ServerFacts,
    ) -> Pairing {
        let clock: Clock = {
            let now = Arc::clone(now);
            Arc::new(move || *now.lock().expect("clock"))
        };
        let database = db::attach(&fixture.layout).expect("attach");
        Pairing::new(
            Arc::new(Mutex::new(database)),
            Arc::clone(secrets),
            facts.clone(),
            clock,
        )
    }

    /// Core restarts: a new process has a new service and a new
    /// connection, and nothing in memory survives.
    fn restart(&mut self) {
        self.pairing = Self::service(&self.fixture, &self.secrets, &self.now, &self.facts);
    }

    fn database(&self) -> Database {
        db::attach(&self.fixture.layout).expect("attach")
    }

    fn now(&self) -> OffsetDateTime {
        *self.now.lock().expect("clock")
    }

    fn advance(&self, by: time::Duration) {
        *self.now.lock().expect("clock") += by;
    }

    fn arm_with(&self, secrets: &SecretsKey) -> PairingSecret {
        arm(
            &mut self.database(),
            secrets,
            &self.facts.server_id,
            self.now(),
        )
        .expect("arm")
        .secret
    }

    fn arm(&self) -> PairingSecret {
        self.arm_with(&self.secrets)
    }

    fn client(&self, conn: &Conn, secret: &PairingSecret) -> ClientPairing {
        ClientPairing::new(
            PairingSecret::from_bytes(*secret.expose()),
            DeviceMetadata::new(DeviceName::parse("Unit").expect("name"), Platform::Other),
            *self.facts.server_id.as_bytes(),
            *self.facts.spki.as_bytes(),
            conn.exporter,
            random(),
        )
    }

    /// `begin` then the client's proof, without completing.
    fn begin(&self, conn: &Conn, secret: &PairingSecret) -> (CompleteRequest, BeginResponse) {
        let client = self.client(conn, secret);
        let begun = self
            .pairing
            .begin(&conn.arrival(), client.begin_request())
            .expect("begin");
        let (request, _) = client.prove(&begun);
        (request, begun)
    }

    fn pair(&self, conn: &Conn, secret: &PairingSecret) -> Result<CompleteResponse, Refused> {
        let client = self.client(conn, secret);
        let begun = self
            .pairing
            .begin(&conn.arrival(), client.begin_request())?;
        let (request, _) = client.prove(&begun);
        self.pairing.complete(&conn.arrival(), &request)
    }

    fn scalar<T: rusqlite::types::FromSql>(&self, sql: &str) -> T {
        self.database()
            .connection()
            .query_row(sql, (), |r| r.get(0))
            .expect(sql)
    }

    fn devices(&self) -> i64 {
        self.scalar("SELECT count(*) FROM devices")
    }

    fn failures(&self) -> i64 {
        self.scalar("SELECT failed_attempts FROM pairing_state")
    }

    /// Whether any part of a sealed secret is stored.
    fn sealed_present(&self) -> bool {
        self.scalar::<i64>(
            "SELECT count(*) FROM pairing_state WHERE secret_nonce IS NOT NULL \
             OR secret_ciphertext IS NOT NULL OR arming_id IS NOT NULL",
        ) > 0
    }
}

#[test]
fn a_restart_during_the_armed_window_still_pairs_until_expiry() {
    let mut harness = Harness::new();
    let secret = harness.arm();
    let conn = Conn::new(1);
    let (before_restart, _) = harness.begin(&conn, &secret);

    harness.restart();
    // The attempt lived in the old process's memory and is gone — refused,
    // and not counted: there is no attempt for it to count against.
    assert_eq!(
        harness
            .pairing
            .complete(&conn.arrival(), &before_restart)
            .err(),
        Some(Refused::Rejected)
    );
    assert_eq!(harness.failures(), 0);
    // The sealed secret survived: a new attempt pairs.
    harness.advance(time::Duration::minutes(10));
    let conn = Conn::new(1);
    harness.pair(&conn, &secret).expect("paired after restart");
    assert_eq!(harness.devices(), 1);
}

#[test]
fn after_a_restart_an_expired_secret_is_still_refused() {
    let mut harness = Harness::new();
    let secret = harness.arm();
    harness.restart();
    harness.advance(super::SECRET_LIFETIME);
    let conn = Conn::new(1);
    let client = harness.client(&conn, &secret);
    assert_eq!(
        harness
            .pairing
            .begin(&conn.arrival(), client.begin_request())
            .err(),
        Some(Refused::Rejected)
    );
    assert!(!harness.sealed_present(), "expired ciphertext deleted");
}

#[test]
fn another_secrets_key_fails_closed_and_never_re_arms() {
    let harness = Harness::new();
    let other = SecretsKey::from_bytes(random());
    // Sealed under another installation's secrets.key.
    let secret = harness.arm_with(&other);
    let before: Vec<u8> = harness.scalar("SELECT secret_ciphertext FROM pairing_state");
    let conn = Conn::new(1);
    assert_eq!(harness.pair(&conn, &secret).err(), Some(Refused::Rejected));
    // Nothing was generated, re-armed or counted; the row is exactly as it
    // was, waiting for the console.
    let after: Vec<u8> = harness.scalar("SELECT secret_ciphertext FROM pairing_state");
    assert_eq!(before, after);
    assert_eq!(harness.failures(), 0);
    assert_eq!(harness.devices(), 0);
}

#[test]
fn tampered_ciphertext_nonce_or_context_is_refused() {
    let harness = Harness::new();
    let flip = |column: &str| {
        let database = harness.database();
        let mut bytes: Vec<u8> = database
            .connection()
            .query_row(&format!("SELECT {column} FROM pairing_state"), (), |r| {
                r.get(0)
            })
            .expect("read");
        bytes[0] ^= 1;
        database
            .connection()
            .execute(
                &format!("UPDATE pairing_state SET {column} = ?1"),
                [bytes.as_slice()],
            )
            .expect("write");
    };
    let set = |sql: &str| {
        harness.database().connection().execute(sql, ()).expect(sql);
    };
    let cases: [(&str, &dyn Fn()); 4] = [
        ("ciphertext", &|| flip("secret_ciphertext")),
        ("nonce", &|| flip("secret_nonce")),
        // A longer life for the same ciphertext.
        ("expiry", &|| {
            set("UPDATE pairing_state SET expires_at = '2099-01-01T00:00:00Z'");
        }),
        ("armed time", &|| {
            set("UPDATE pairing_state SET armed_at = '2000-01-01T00:00:00Z'");
        }),
    ];
    for (case, tamper) in cases {
        let secret = harness.arm();
        tamper();
        let conn = Conn::new(1);
        assert_eq!(
            harness.pair(&conn, &secret).err(),
            Some(Refused::Rejected),
            "{case}"
        );
        assert_eq!(harness.devices(), 0, "{case}");
        assert_eq!(harness.failures(), 0, "{case}: not the client's failure");
    }
}

#[test]
fn consumed_expired_and_replaced_secrets_leave_nothing_to_recover() {
    let harness = Harness::new();
    // Consumed.
    let secret = harness.arm();
    assert!(harness.sealed_present());
    harness.pair(&Conn::new(1), &secret).expect("paired");
    assert!(!harness.sealed_present());
    // Expired, deleted by the sweep with nobody asking.
    harness.arm();
    harness.advance(super::SECRET_LIFETIME);
    assert!(harness
        .database()
        .disarm_if_expired(harness.now())
        .expect("sweep"));
    assert!(!harness.sealed_present());
    // Failures end nothing (ADR-020): the sealed secret stays until one of
    // the events above.
    harness.arm();
    let wrong = PairingSecret::from_bytes(random());
    for _ in 0..20 {
        assert!(harness.pair(&Conn::new(1), &wrong).is_err());
    }
    assert!(harness.sealed_present());
    assert_eq!(harness.failures(), 20);
    // Replaced: the old ciphertext is overwritten by the new arming.
    let first = harness.arm();
    let first_ciphertext: Vec<u8> = harness.scalar("SELECT secret_ciphertext FROM pairing_state");
    let second = harness.arm();
    let second_ciphertext: Vec<u8> = harness.scalar("SELECT secret_ciphertext FROM pairing_state");
    assert_ne!(first_ciphertext, second_ciphertext);
    assert!(harness.pair(&Conn::new(1), &first).is_err());
    harness
        .pair(&Conn::new(1), &second)
        .expect("the new one works");
}

#[test]
fn one_secret_never_makes_two_devices_even_concurrently() {
    let harness = Arc::new(Harness::new());
    let secret = harness.arm();
    // Two attempts from two sources, each with a valid proof.
    let one = Conn::new(1);
    let two = Conn::new(2);
    let (first, _) = harness.begin(&one, &secret);
    let (second, _) = harness.begin(&two, &secret);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let run = |conn: Conn, request: CompleteRequest| {
        let harness = Arc::clone(&harness);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            harness.pairing.complete(&conn.arrival(), &request).is_ok()
        })
    };
    let a = run(one, first);
    let b = run(two, second);
    let outcomes = [a.join().expect("thread"), b.join().expect("thread")];
    assert_eq!(outcomes.iter().filter(|ok| **ok).count(), 1, "{outcomes:?}");
    assert_eq!(harness.devices(), 1);

    // The same attempt twice at once: one success at most.
    let secret = harness.arm();
    let conn = Arc::new(Conn::new(3));
    let (request, _) = harness.begin(&conn, &secret);
    let request = Arc::new(request);
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let harness = Arc::clone(&harness);
            let barrier = Arc::clone(&barrier);
            let conn = Arc::clone(&conn);
            let request = Arc::clone(&request);
            std::thread::spawn(move || {
                barrier.wait();
                harness.pairing.complete(&conn.arrival(), &request).is_ok()
            })
        })
        .collect();
    let wins = threads
        .into_iter()
        .map(|t| t.join().expect("thread"))
        .filter(|ok| *ok)
        .count();
    assert_eq!(wins, 1);
    assert_eq!(harness.devices(), 2);
}

#[test]
fn a_crash_before_commit_leaves_the_secret_unused_and_no_device() {
    let harness = Harness::new();
    let secret = harness.arm();
    {
        // Everything `complete` writes, then the process dies before the
        // commit: the transaction is dropped.
        let mut database = harness.database();
        let transaction = database.pairing_transaction().expect("transaction");
        transaction
            .consume(
                &crate::db::pairing::NewDevice {
                    device_id: "00112233445566778899aabbccddeeff",
                    name: "Ghost",
                    platform: "linux",
                    token_digest: &[7; 32],
                },
                harness.now(),
            )
            .expect("consume");
        drop(transaction);
    }
    assert_eq!(harness.devices(), 0);
    assert!(harness.sealed_present(), "still armed");
    assert_eq!(
        harness.scalar::<i64>("SELECT claimed FROM pairing_state"),
        0
    );
    // The real attempt then succeeds exactly once.
    harness.pair(&Conn::new(1), &secret).expect("paired");
    assert!(harness.pair(&Conn::new(1), &secret).is_err());
    assert_eq!(harness.devices(), 1);
    assert_eq!(
        harness.scalar::<i64>("SELECT claimed FROM pairing_state"),
        1
    );
}

#[test]
fn a_failed_device_insert_rolls_the_consumption_back() {
    let harness = Harness::new();
    let secret = harness.arm();
    let mut database = harness.database();
    // A device whose verifier collides makes the insert fail inside the
    // transaction; the disarm and the claim in the same transaction must
    // not survive it.
    database
        .connection()
        .execute(
            "INSERT INTO devices (device_id, name, platform, role, token_digest, created_at) \
             VALUES ('ffeeddccbbaa99887766554433221100', 'Old', 'linux', 'owner', ?1, \
             '2026-01-01T00:00:00Z')",
            [[9_u8; 32].as_slice()],
        )
        .expect("seed");
    let transaction = database.pairing_transaction().expect("transaction");
    let failed = transaction.consume(
        &crate::db::pairing::NewDevice {
            device_id: "00112233445566778899aabbccddeeff",
            name: "Clash",
            platform: "linux",
            token_digest: &[9; 32],
        },
        harness.now(),
    );
    assert!(failed.is_err());
    drop(transaction);
    drop(database);
    assert!(harness.sealed_present(), "still armed");
    assert_eq!(
        harness.scalar::<i64>("SELECT claimed FROM pairing_state"),
        0
    );
    harness.pair(&Conn::new(1), &secret).expect("paired");
}

#[test]
fn attempts_are_bounded_and_expire() {
    let harness = Harness::new();
    let secret = harness.arm();
    let conns: Vec<Conn> = (0..u32::try_from(MAX_ATTEMPTS).expect("small"))
        .map(Conn::new)
        .collect();
    for conn in &conns {
        harness.begin(conn, &secret);
    }
    assert_eq!(harness.pairing.attempts_in_progress(), MAX_ATTEMPTS);
    let late = Conn::new(99);
    let client = harness.client(&late, &secret);
    assert_eq!(
        harness
            .pairing
            .begin(&late.arrival(), client.begin_request())
            .err(),
        Some(Refused::Rejected)
    );
    // Many refused begins create nothing and count nothing.
    for n in 100..200 {
        let conn = Conn::new(n);
        let client = harness.client(&conn, &secret);
        let _ = harness
            .pairing
            .begin(&conn.arrival(), client.begin_request());
    }
    assert_eq!(harness.pairing.attempts_in_progress(), MAX_ATTEMPTS);
    assert_eq!(harness.failures(), 0);
    // Once the window passes, the slots free up.
    harness.advance(ATTEMPT_WINDOW);
    harness.pair(&late, &secret).expect("a slot after expiry");
}

#[test]
fn a_connection_without_an_exporter_cannot_pair() {
    let harness = Harness::new();
    let secret = harness.arm();
    let conn = Conn::new(1);
    let client = harness.client(&conn, &secret);
    let arrival = Arrival {
        connection: conn.id,
        source: conn.source,
        exporter: None,
    };
    assert_eq!(
        harness
            .pairing
            .begin(&arrival, client.begin_request())
            .err(),
        Some(Refused::Rejected)
    );
}

#[test]
fn the_armed_profile_comes_from_the_state_not_the_request() {
    let harness = Harness::new();
    let secret = harness.arm();
    let stored: String = harness.scalar("SELECT profile FROM pairing_state");
    assert_eq!(stored, atrium_pairing::BINDING_PROFILE_NATIVE);
    // The schema cannot hold any other profile.
    let refused = harness.database().connection().execute(
        "UPDATE pairing_state SET profile = 'atrium-pair-binding/web-pki-v1'",
        (),
    );
    assert!(refused.is_err());
    harness.pair(&Conn::new(1), &secret).expect("native pairs");
}
