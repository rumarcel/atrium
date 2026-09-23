//! `atrium-core init-identity`: the one place a server identity is created.
//!
//! Runs once, at installation, as the `atrium` user, while the installer has
//! left `/etc/atrium` owned by `atrium` and mode `0700`. Afterwards the
//! installer hands the directory and the three files to root
//! (`docs/M1-IMPLEMENTATION-PLAN.md` section 12.2, step 8), and from then on
//! nothing running as `atrium` — this command included — can write there.
//!
//! Before writing anything it classifies what is already present:
//!
//! - **fresh** — no identity file, no leftover temporary, no state database:
//!   generate.
//! - **initialized** — a complete identity that verifies: change nothing,
//!   report it, and exit non-zero so an installer cannot mistake "already
//!   there" for "just created".
//! - **partial** or **inconsistent** — anything else: refuse, change nothing,
//!   and say exactly what was found. An interrupted installation is repaired
//!   by a person who has looked at it, not by this command guessing.
//!
//! Generation order, chosen so an interruption at any point is recognisable:
//! `tls.key`, `secrets.key`, the state database, the certificate, and
//! `identity.json` **last**. Each file is created exclusively, so nothing is
//! ever overwritten. If a step fails, the files this run created are removed
//! again, so a failed first attempt leaves a fresh tree behind; a crash
//! instead leaves a partial one, which the next run refuses.

use std::fmt;
use std::io;

use time::OffsetDateTime;

use crate::certificate::{self, AddressSet, CertificateRecord, SubjectNames};
use crate::db;
use crate::fsio::{self, Spec};
use crate::identity::{
    self, DeviceKey, Identity, IdentityFault, IdentityRecord, Inconsistency, Protection, ServerId,
    SpkiPin, SECRETS_KEY_LEN,
};
use crate::layout::{
    Layout, CERTIFICATE_FILE, CERTIFICATE_STATE_FILE, IDENTITY_FILE, PRIVATE_KEY_FILE,
    SECRETS_KEY_FILE,
};
use crate::protect::{self, Expect, Violation};
use crate::redact::Redacted;

/// What an initialization run found and did.
#[derive(Debug)]
pub enum InitOutcome {
    /// A new identity was created.
    Created {
        /// The new identifier.
        server_id: ServerId,
        /// The new key's pin.
        pin: SpkiPin,
        /// The first certificate.
        certificate: CertificateRecord,
    },
    /// A complete, consistent identity already existed. Nothing was changed.
    AlreadyInitialized {
        /// The existing identifier.
        server_id: ServerId,
        /// The existing key's pin.
        pin: SpkiPin,
    },
}

/// Why initialization refused to act.
#[derive(Debug)]
pub enum InitRefusal {
    /// The directories are not in the state the installer leaves them in.
    Precondition(Violation),
    /// Something is present, but not a complete, consistent identity.
    Inconsistent(Inconsistency),
    /// An identity file is present but unreadable.
    Unreadable(&'static str),
    /// A state database exists without an identity — left over from a
    /// previous installation. Pairing it with a new identity would strand
    /// every device it lists.
    StateWithoutIdentity,
    /// Generation or writing failed part-way; this run's files were removed.
    Failed {
        /// Which step.
        step: &'static str,
        /// What happened, without any key material.
        reason: String,
    },
}

impl fmt::Display for InitRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Precondition(violation) => write!(
                formatter,
                "the installation tree is not ready for init-identity: {violation}"
            ),
            Self::Inconsistent(detail) => write!(
                formatter,
                "refusing to touch an existing identity that is incomplete or inconsistent \
                 ({detail}); nothing was changed. Inspect the directory and remove the \
                 leftovers by hand only if you are sure no client ever paired with them"
            ),
            Self::Unreadable(file) => write!(formatter, "{file} exists but cannot be read"),
            Self::StateWithoutIdentity => formatter.write_str(
                "a state database exists but no identity does; refusing to create a new \
                 identity for an old database. Move the old state directory aside first",
            ),
            Self::Failed { step, reason } => write!(
                formatter,
                "initialization failed while {step}: {reason}; the files this run created \
                 were removed"
            ),
        }
    }
}

impl std::error::Error for InitRefusal {}

/// The installer leaves both directories owned by the service user and closed
/// to everyone else before this runs.
fn check_preconditions(layout: &Layout) -> Result<(), InitRefusal> {
    let euid = protect::euid();
    protect::check(layout.etc_dir(), Expect::PrivateDirectory, euid)
        .map_err(InitRefusal::Precondition)?;
    protect::check(layout.state_dir(), Expect::PrivateDirectory, euid)
        .map_err(InitRefusal::Precondition)
}

fn state_exists(layout: &Layout) -> Result<bool, InitRefusal> {
    let database = layout.database();
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let path = std::path::PathBuf::from(format!("{}{suffix}", database.display()));
        match fsio::lstat(&path) {
            Ok(Some(_)) => return Ok(true),
            Ok(None) => {}
            Err(_) => return Err(InitRefusal::Unreadable("state database")),
        }
    }
    Ok(false)
}

/// Classifies the tree and, only if it is fresh, creates the identity.
///
/// # Errors
///
/// An [`InitRefusal`]. On every refusal the tree is as it was found.
pub fn run(
    layout: &Layout,
    addresses: &AddressSet,
    now: OffsetDateTime,
) -> Result<InitOutcome, InitRefusal> {
    let inventory = identity::inventory(layout).map_err(|fault| match fault {
        IdentityFault::Unreadable(file) => InitRefusal::Unreadable(file),
        other => InitRefusal::Failed {
            step: "inspecting the identity directory",
            reason: other.to_string(),
        },
    })?;

    if !inventory.is_empty() {
        return match identity::load(layout, Protection::NotChecked) {
            Ok(existing) => Ok(InitOutcome::AlreadyInitialized {
                server_id: existing.server_id(),
                pin: existing.pin(),
            }),
            Err(IdentityFault::Inconsistent(detail)) => Err(InitRefusal::Inconsistent(detail)),
            Err(IdentityFault::Unreadable(file)) => Err(InitRefusal::Unreadable(file)),
            Err(IdentityFault::Missing) => {
                Err(InitRefusal::Inconsistent(Inconsistency::LeftoverTemporary))
            }
            Err(IdentityFault::Unprotected(violation)) => Err(InitRefusal::Precondition(violation)),
        };
    }
    // Only generation needs the pre-lockdown tree. Recognising an existing
    // identity must work after lockdown too, so a second run on a finished
    // installation reports "already initialized" rather than a permissions
    // complaint.
    check_preconditions(layout)?;
    if state_exists(layout)? {
        return Err(InitRefusal::StateWithoutIdentity);
    }

    let mut created = Created::new(layout);
    match generate(layout, addresses, now, &mut created) {
        Ok(outcome) => {
            created.keep();
            Ok(outcome)
        }
        Err(refusal) => Err(refusal),
    }
}

/// Files this run created, removed on drop unless kept.
struct Created<'a> {
    layout: &'a Layout,
    etc: Vec<&'static str>,
    state: Vec<&'static str>,
    database: bool,
    keep: bool,
}

impl<'a> Created<'a> {
    fn new(layout: &'a Layout) -> Self {
        Self {
            layout,
            etc: Vec::new(),
            state: Vec::new(),
            database: false,
            keep: false,
        }
    }

    fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for Created<'_> {
    fn drop(&mut self) {
        if self.keep {
            return;
        }
        for name in self.etc.iter().rev() {
            let _ = std::fs::remove_file(self.layout.etc_file(name));
        }
        for name in self.state.iter().rev() {
            let _ = std::fs::remove_file(self.layout.state_file(name));
        }
        if self.database {
            let database = self.layout.database();
            for suffix in ["-wal", "-shm", "-journal", ""] {
                let _ = std::fs::remove_file(format!("{}{suffix}", database.display()));
            }
        }
        let _ = fsio::sync_dir(self.layout.etc_dir());
        let _ = fsio::sync_dir(self.layout.state_dir());
    }
}

fn failed(step: &'static str) -> impl Fn(io::Error) -> InitRefusal {
    move |error| InitRefusal::Failed {
        step,
        reason: error.kind().to_string(),
    }
}

fn generate(
    layout: &Layout,
    addresses: &AddressSet,
    now: OffsetDateTime,
    created: &mut Created<'_>,
) -> Result<InitOutcome, InitRefusal> {
    let server_id = ServerId::generate().map_err(|_| InitRefusal::Failed {
        step: "drawing server_id",
        reason: "the operating system supplied no randomness".to_owned(),
    })?;
    let key = DeviceKey::generate().map_err(|error| InitRefusal::Failed {
        step: "generating the identity key",
        reason: error.to_string(),
    })?;
    let mut secrets = [0_u8; SECRETS_KEY_LEN];
    getrandom::fill(&mut secrets).map_err(|_| InitRefusal::Failed {
        step: "drawing secrets.key",
        reason: "the operating system supplied no randomness".to_owned(),
    })?;
    let secrets = Redacted::new(secrets);

    let etc = layout.etc_dir();
    fsio::create_new(
        etc,
        PRIVATE_KEY_FILE,
        key.to_pkcs8().expose(),
        Spec::mode(0o600),
    )
    .map_err(failed("writing tls.key"))?;
    created.etc.push(PRIVATE_KEY_FILE);

    fsio::create_new(etc, SECRETS_KEY_FILE, secrets.expose(), Spec::mode(0o600))
        .map_err(failed("writing secrets.key"))?;
    created.etc.push(SECRETS_KEY_FILE);

    match db::create(layout) {
        Ok(mut database) => {
            created.database = true;
            // Bind the new state to the new key from the first moment.
            database
                .reconcile_identity(&key.pin(), db::Actor::System, now)
                .map_err(|error| InitRefusal::Failed {
                    step: "recording the identity in the state database",
                    reason: error.to_string(),
                })?;
            drop(database);
        }
        // Checked for above; if one appeared since, it is not ours to remove.
        Err(db::StateFault::AlreadyExists) => return Err(InitRefusal::StateWithoutIdentity),
        Err(fault) => {
            created.database = true;
            return Err(InitRefusal::Failed {
                step: "creating the state database",
                reason: fault.to_string(),
            });
        }
    }

    let identity = Identity {
        record: IdentityRecord {
            server_id,
            created_at: now,
            spki: key.pin(),
        },
        key,
    };
    let names = SubjectNames::for_server(&server_id, addresses);
    let issued =
        certificate::issue(&identity, &names, now).map_err(|error| InitRefusal::Failed {
            step: "issuing the first certificate",
            reason: error.to_string(),
        })?;
    created.state.push(CERTIFICATE_FILE);
    created.state.push(CERTIFICATE_STATE_FILE);
    certificate::write(layout, &issued).map_err(|error| InitRefusal::Failed {
        step: "writing the first certificate",
        reason: error.to_string(),
    })?;

    // Last: its presence is what says the identity is complete.
    fsio::create_new(
        etc,
        IDENTITY_FILE,
        &identity.record.to_json(),
        Spec::mode(0o600),
    )
    .map_err(failed("writing identity.json"))?;
    created.etc.push(IDENTITY_FILE);

    // Read everything back through the same path Core will use.
    let reloaded =
        identity::load(layout, Protection::NotChecked).map_err(|fault| InitRefusal::Failed {
            step: "verifying the written identity",
            reason: fault.to_string(),
        })?;
    if reloaded.server_id() != server_id || reloaded.pin() != identity.pin() {
        return Err(InitRefusal::Failed {
            step: "verifying the written identity",
            reason: "what was read back differs from what was written".to_owned(),
        });
    }

    Ok(InitOutcome::Created {
        server_id,
        pin: identity.pin(),
        certificate: issued.record,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{chmod, Fixture};

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time")
    }

    fn addresses() -> AddressSet {
        AddressSet::new(["192.0.2.10".parse().expect("ip")])
    }

    fn init(fixture: &Fixture) -> Result<InitOutcome, InitRefusal> {
        run(&fixture.layout, &addresses(), now())
    }

    fn created(fixture: &Fixture) -> (ServerId, SpkiPin) {
        match init(fixture).expect("fresh init") {
            InitOutcome::Created { server_id, pin, .. } => (server_id, pin),
            other => panic!("expected a new identity, got {other:?}"),
        }
    }

    #[test]
    fn identity_round_trip() {
        let fixture = Fixture::new("roundtrip");
        let (server_id, pin) = created(&fixture);
        let loaded = identity::load(&fixture.layout, Protection::NotChecked).expect("loads");
        assert_eq!(loaded.server_id(), server_id);
        assert_eq!(loaded.pin(), pin);
        assert_eq!(loaded.record.spki, pin);
    }

    #[test]
    fn init_writes_every_file_with_its_mode() {
        let fixture = Fixture::new("modes");
        created(&fixture);
        let layout = &fixture.layout;
        for (path, mode) in [
            (layout.etc_file(PRIVATE_KEY_FILE), 0o600),
            (layout.etc_file(SECRETS_KEY_FILE), 0o600),
            (layout.etc_file(IDENTITY_FILE), 0o600),
            (layout.state_file(CERTIFICATE_FILE), 0o644),
            (layout.state_file(CERTIFICATE_STATE_FILE), 0o600),
            (layout.database(), 0o600),
        ] {
            let metadata = std::fs::metadata(&path).expect("exists");
            assert_eq!(fsio::mode_of(&metadata), mode, "{}", path.display());
        }
        let secrets = std::fs::read(layout.etc_file(SECRETS_KEY_FILE)).expect("read");
        assert_eq!(secrets.len(), SECRETS_KEY_LEN);
        assert_ne!(secrets, vec![0_u8; SECRETS_KEY_LEN]);
        let database = db::open(layout, now()).expect("opens");
        assert_eq!(
            database.database.schema_version().expect("version"),
            db::SCHEMA_VERSION
        );
    }

    #[test]
    fn a_second_run_changes_nothing() {
        let fixture = Fixture::new("twice");
        let (server_id, pin) = created(&fixture);
        let before = fixture.snapshot();
        match init(&fixture).expect("second run") {
            InitOutcome::AlreadyInitialized {
                server_id: again,
                pin: pin_again,
            } => {
                assert_eq!(again, server_id);
                assert_eq!(pin_again, pin);
            }
            other => panic!("a second run must not create anything: {other:?}"),
        }
        assert_eq!(fixture.snapshot(), before, "not one byte may change");
    }

    #[test]
    fn two_installations_get_different_identities() {
        let first = Fixture::new("first");
        let second = Fixture::new("second");
        assert_ne!(created(&first), created(&second));
    }

    #[test]
    fn partial_identities_are_refused_and_left_alone() {
        // Every non-empty strict subset of the three files.
        let subsets: [&[&str]; 6] = [
            &[PRIVATE_KEY_FILE],
            &[SECRETS_KEY_FILE],
            &[IDENTITY_FILE],
            &[PRIVATE_KEY_FILE, SECRETS_KEY_FILE],
            &[PRIVATE_KEY_FILE, IDENTITY_FILE],
            &[SECRETS_KEY_FILE, IDENTITY_FILE],
        ];
        for keep in subsets {
            let fixture = Fixture::new("partial");
            created(&fixture);
            for name in crate::layout::IDENTITY_FILES {
                if !keep.contains(&name) {
                    std::fs::remove_file(fixture.layout.etc_file(name)).expect("remove");
                }
            }
            let before = fixture.snapshot();
            let refusal = init(&fixture).expect_err("partial identity must be refused");
            assert!(
                matches!(
                    refusal,
                    InitRefusal::Inconsistent(Inconsistency::Partial { .. })
                ),
                "{keep:?}: {refusal:?}"
            );
            assert_eq!(fixture.snapshot(), before, "{keep:?}: nothing may change");

            let fault = identity::load(&fixture.layout, Protection::NotChecked)
                .expect_err("Core must refuse it too");
            assert!(matches!(
                fault,
                IdentityFault::Inconsistent(Inconsistency::Partial { .. })
            ));
        }
    }

    #[test]
    fn corrupt_identities_are_refused_and_left_alone() {
        let damage: [(&str, &str, &[u8]); 4] = [
            ("record", IDENTITY_FILE, b"{ not json"),
            ("key", PRIVATE_KEY_FILE, b"\x30\x03\x02\x01\x00"),
            ("secrets", SECRETS_KEY_FILE, b"short"),
            ("empty key", PRIVATE_KEY_FILE, b""),
        ];
        for (what, file, bytes) in damage {
            let fixture = Fixture::new("corrupt");
            created(&fixture);
            std::fs::write(fixture.layout.etc_file(file), bytes).expect("damage");
            let before = fixture.snapshot();
            let refusal = init(&fixture).expect_err("corrupt identity must be refused");
            assert!(
                matches!(refusal, InitRefusal::Inconsistent(_)),
                "{what}: {refusal:?}"
            );
            assert_eq!(fixture.snapshot(), before, "{what}: nothing may change");
        }
    }

    #[test]
    fn a_replaced_key_does_not_match_its_record() {
        let fixture = Fixture::new("swapped");
        created(&fixture);
        let other = DeviceKey::generate().expect("keygen");
        std::fs::write(
            fixture.layout.etc_file(PRIVATE_KEY_FILE),
            other.to_pkcs8().expose(),
        )
        .expect("swap");
        assert_eq!(
            identity::load(&fixture.layout, Protection::NotChecked).map(|_| ()),
            Err(IdentityFault::Inconsistent(
                Inconsistency::KeyDoesNotMatchRecord
            ))
        );
    }

    #[test]
    fn a_leftover_temporary_file_is_evidence_of_an_interrupted_write() {
        let fixture = Fixture::new("leftover");
        std::fs::write(fixture.layout.etc_file(".atrium-tmp-tls.key-00"), b"half").expect("seed");
        let before = fixture.snapshot();
        assert!(matches!(
            init(&fixture),
            Err(InitRefusal::Inconsistent(Inconsistency::LeftoverTemporary))
        ));
        assert_eq!(fixture.snapshot(), before);
    }

    #[test]
    fn an_old_database_without_an_identity_is_refused() {
        let fixture = Fixture::new("olddb");
        std::fs::write(fixture.layout.database(), b"").expect("seed");
        chmod(&fixture.layout.database(), 0o600);
        assert!(matches!(
            init(&fixture),
            Err(InitRefusal::StateWithoutIdentity)
        ));
        assert!(!fixture.layout.etc_file(PRIVATE_KEY_FILE).exists());
    }

    #[test]
    fn a_failed_run_removes_what_it_created() {
        let fixture = Fixture::new("rollback");
        // The certificate cannot be written: a directory sits where it goes.
        std::fs::create_dir(fixture.layout.state_file(CERTIFICATE_FILE)).expect("block");
        assert!(matches!(init(&fixture), Err(InitRefusal::Failed { .. })));
        for name in crate::layout::IDENTITY_FILES {
            assert!(
                !fixture.layout.etc_file(name).exists(),
                "{name} left behind"
            );
        }
        assert!(!fixture.layout.database().exists(), "database left behind");
        std::fs::remove_dir(fixture.layout.state_file(CERTIFICATE_FILE)).expect("unblock");
        created(&fixture);
    }

    #[test]
    fn an_exposed_tree_is_refused_before_anything_is_written() {
        let fixture = Fixture::new("exposed");
        chmod(fixture.layout.etc_dir(), 0o755);
        assert!(matches!(init(&fixture), Err(InitRefusal::Precondition(_))));
        assert!(fixture.snapshot().is_empty());
    }
}
