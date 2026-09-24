//! Pairing (M1E): arming at the console, and `pair/info`, `pair/begin` and
//! `pair/complete` over the network, exactly as ADR-003 §3–§6, plan §6,
//! ADR-019 and ADR-020 specify.
//!
//! # States
//!
//! The **server** state is the `pairing_state` row, and only the row:
//!
//! | State | Row | Leaves by |
//! | --- | --- | --- |
//! | Unclaimed, disarmed | `claimed=0, armed=0` | `atriumctl pair` |
//! | Armed (claimed or not) | `armed=1`, sealed secret present | success, expiry, re-arm, identity change |
//! | Claimed, disarmed | `claimed=1, armed=0` | `atriumctl pair` |
//!
//! There is no failure-locked state (ADR-020): a failed attempt never ends
//! the armed secret.
//!
//! Expiry is enforced on every read (an expired arming is never usable) and
//! the ciphertext is deleted by whichever comes first: the next pairing
//! request, or Core's periodic sweep.
//!
//! An **attempt** is `Begun` from a successful `begin` until it is removed,
//! which is the first `complete` naming it — success or not — or its expiry.
//! `Completed`, `Failed` and `Expired` are therefore not stored: a removed
//! attempt cannot be completed, and that is the whole of what those states
//! mean.
//!
//! # Who owns what
//!
//! - `pairing_state` (armed secret, failure count, claim) is owned by
//!   the database; every decision that changes it runs in one `IMMEDIATE`
//!   transaction, so two requests — or Core and `atriumctl pair` — cannot
//!   interleave inside a decision.
//! - Attempts are owned by [`Pairing`], in memory, at most [`MAX_ATTEMPTS`],
//!   one per source. Each records the [`ConnectionId`] of the TLS connection
//!   its `begin` arrived on. The exporter is **not** copied into it: at
//!   `complete` the exporter is read from the connection the `complete`
//!   arrived on, and the attempt is refused unless that connection's id is
//!   the recorded one. Because HTTP/1.1 serves one request at a time per
//!   connection and the attempt is removed before anything else is checked,
//!   two requests can never both use one attempt, and no attempt can be
//!   completed from any connection but its own.
//! - The secret's plaintext exists only inside [`Pairing::complete`], for the
//!   proof computation, as a zeroizing value.
//!
//! # Failures
//!
//! Every refusal is the same `pairing.rejected`. The log carries a closed
//! reason; nothing carries a secret, a proof, a nonce, an exporter, a token
//! or a token digest. A `complete` that names a live attempt for the current
//! arming and does not succeed consumes that attempt and increments the
//! arming's `failed_attempts`, which the audit row ending the arming records
//! and nothing else reads (ADR-020). The armed secret is untouched: 128
//! random bits need no lockout, and a lockout would let any LAN client end
//! the owner's pairing window. The proof is always computed, with placeholder inputs when there
//! is nothing real to compute it over, so the refusal paths do comparable
//! work; no claim of network-level constant time is made.

pub mod seal;

use std::sync::{Arc, Mutex, MutexGuard};

use time::{Duration, OffsetDateTime};
use zeroize::Zeroizing;

use atrium_pairing::device::DeviceMetadata;
use atrium_pairing::secret::PairingSecret;
use atrium_pairing::token::DeviceToken;
use atrium_pairing::transcript::{self, Profile, TranscriptInputs};
use atrium_pairing::wire::{
    BeginRequest, BeginResponse, Bytes, CompleteRequest, CompleteResponse, Id, InfoResponse,
    ServerRef,
};

use crate::db::pairing::{ArmedRow, NewDevice, NATIVE_PROFILE};
use crate::db::{Actor, Database};
use crate::http::connection::ConnectionId;
use crate::http::limits::SourceKey;
use crate::identity::{format_time, SecretsKey, ServerId, SpkiPin};

/// How long an armed secret lasts.
pub const SECRET_LIFETIME: Duration = Duration::minutes(15);
/// How long an attempt has from `begin` to `complete`.
pub const ATTEMPT_WINDOW: Duration = Duration::minutes(2);
/// Attempts in progress at once, across all sources.
pub const MAX_ATTEMPTS: usize = 4;

/// Wall-clock time, injectable so tests can move it.
pub type Clock = Arc<dyn Fn() -> OffsetDateTime + Send + Sync>;

/// The real clock.
#[must_use]
pub fn system_clock() -> Clock {
    Arc::new(OffsetDateTime::now_utc)
}

/// Why a request was refused. The network sees one answer for
/// [`Refused::Rejected`], whatever the reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// `pairing.rejected`.
    Rejected,
    /// Core could not do its part (database, randomness).
    Internal,
}

/// What the server says about itself.
#[derive(Debug, Clone)]
pub struct ServerFacts {
    /// From `identity.json`.
    pub server_id: ServerId,
    /// Of the identity key.
    pub spki: SpkiPin,
    /// From `core.toml`.
    pub name: String,
}

/// The connection a request arrived on.
#[derive(Debug, Clone, Copy)]
pub struct Arrival<'a> {
    /// Its id.
    pub connection: ConnectionId,
    /// Its source.
    pub source: SourceKey,
    /// Its TLS 1.3 exporter, if it has one.
    pub exporter: Option<&'a [u8; 32]>,
}

struct Attempt {
    pairing_id: [u8; 16],
    connection: ConnectionId,
    source: SourceKey,
    arming_id: [u8; 16],
    client_nonce: [u8; 32],
    server_nonce: [u8; 32],
    device: DeviceMetadata,
    expires_at: OffsetDateTime,
}

/// Closed reasons, for the log and the audit only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reason {
    NotOpen,
    ProfileNotPermitted,
    NoBinding,
    Busy,
    UnknownAttempt,
    OtherConnection,
    AttemptExpired,
    ArmingGone,
    Unsealable,
    ProofMismatch,
}

impl Reason {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotOpen => "not_open",
            Self::ProfileNotPermitted => "profile_not_permitted",
            Self::NoBinding => "no_channel_binding",
            Self::Busy => "attempts_full",
            Self::UnknownAttempt => "unknown_attempt",
            Self::OtherConnection => "other_connection",
            Self::AttemptExpired => "attempt_expired",
            Self::ArmingGone => "arming_gone",
            Self::Unsealable => "secret_unsealable",
            Self::ProofMismatch => "proof_mismatch",
        }
    }
}

fn rejected(route: &'static str, reason: Reason) -> Refused {
    tracing::info!(
        event = "pairing_rejected",
        component = "atrium-core",
        route = route,
        reason = reason.as_str(),
        "a pairing request was refused"
    );
    Refused::Rejected
}

fn internal(route: &'static str, what: &'static str, error: &dyn std::fmt::Display) -> Refused {
    tracing::error!(
        event = "pairing_internal_error",
        component = "atrium-core",
        route = route,
        step = what,
        reason = %error,
        "a pairing request could not be served"
    );
    Refused::Internal
}

fn random<const N: usize>() -> Result<[u8; N], getrandom::Error> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes)?;
    Ok(bytes)
}

/// The network half of pairing.
pub struct Pairing {
    database: Arc<Mutex<Database>>,
    secrets: Arc<SecretsKey>,
    facts: ServerFacts,
    attempts: Mutex<Vec<Attempt>>,
    clock: Clock,
}

impl std::fmt::Debug for Pairing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Pairing")
            .field("facts", &self.facts)
            .finish_non_exhaustive()
    }
}

impl Pairing {
    /// Pairing over `database`, opening sealed secrets with `secrets`.
    #[must_use]
    pub fn new(
        database: Arc<Mutex<Database>>,
        secrets: Arc<SecretsKey>,
        facts: ServerFacts,
        clock: Clock,
    ) -> Self {
        Self {
            database,
            secrets,
            facts,
            attempts: Mutex::new(Vec::new()),
            clock,
        }
    }

    fn database(&self) -> Result<MutexGuard<'_, Database>, Refused> {
        self.database.lock().map_err(|_| Refused::Internal)
    }

    fn attempts(&self) -> Result<MutexGuard<'_, Vec<Attempt>>, Refused> {
        self.attempts.lock().map_err(|_| Refused::Internal)
    }

    /// `GET /api/v1/pair/info`: display material only. Reads; never writes.
    ///
    /// # Errors
    ///
    /// [`Refused::Internal`] when the state cannot be read.
    pub fn info(&self) -> Result<InfoResponse, Refused> {
        let now = (self.clock)();
        let row = self
            .database()?
            .pairing_row()
            .map_err(|error| internal("pair-info", "read", &error))?;
        let open = row.armed.as_ref().is_some_and(|a| a.expires_at > now);
        Ok(InfoResponse {
            server_id: Id(*self.facts.server_id.as_bytes()),
            name: self.facts.name.clone(),
            version: crate::VERSION.to_owned(),
            spki: self.facts.spki.to_string(),
            claimed: row.claimed,
            pairing_open: open,
        })
    }

    /// `POST /api/v1/pair/begin`.
    ///
    /// # Errors
    ///
    /// [`Refused::Rejected`] for every refusal; [`Refused::Internal`] when
    /// Core cannot do its part.
    pub fn begin(
        &self,
        arrival: &Arrival<'_>,
        request: BeginRequest,
    ) -> Result<BeginResponse, Refused> {
        const ROUTE: &str = "pair-begin";
        let now = (self.clock)();
        // The profile is the armed one or nothing; the request can only fail
        // to match it, never choose it.
        let Some(profile) = Profile::parse(&request.binding_profile.0) else {
            return Err(rejected(ROUTE, Reason::ProfileNotPermitted));
        };
        if arrival.exporter.is_none() {
            return Err(rejected(ROUTE, Reason::NoBinding));
        }
        let armed = {
            let mut database = self.database()?;
            database
                .disarm_if_expired(now)
                .map_err(|error| internal(ROUTE, "expire", &error))?;
            let row = database
                .pairing_row()
                .map_err(|error| internal(ROUTE, "read", &error))?;
            match row.armed {
                Some(armed) if armed.expires_at > now => armed,
                _ => return Err(rejected(ROUTE, Reason::NotOpen)),
            }
        };
        if armed.profile != profile.id() {
            return Err(rejected(ROUTE, Reason::ProfileNotPermitted));
        }

        let mut attempts = self.attempts()?;
        attempts.retain(|a| a.expires_at > now && a.source != arrival.source);
        if attempts.len() >= MAX_ATTEMPTS {
            return Err(rejected(ROUTE, Reason::Busy));
        }
        let pairing_id = random::<16>().map_err(|error| internal(ROUTE, "random", &error))?;
        let server_nonce = random::<32>().map_err(|error| internal(ROUTE, "random", &error))?;
        let expires_at = (now + ATTEMPT_WINDOW).min(armed.expires_at);
        attempts.push(Attempt {
            pairing_id,
            connection: arrival.connection,
            source: arrival.source,
            arming_id: armed.arming_id,
            client_nonce: request.client_nonce.0,
            server_nonce,
            device: DeviceMetadata::new(request.device_name, request.platform),
            expires_at,
        });
        tracing::info!(
            event = "pairing_begun",
            component = "atrium-core",
            platform = request.platform.as_str(),
            "a pairing attempt began"
        );
        Ok(BeginResponse {
            pairing_id: Id(pairing_id),
            server_nonce: Bytes(server_nonce),
            expires_at: format_time(expires_at),
        })
    }

    /// `POST /api/v1/pair/complete`.
    ///
    /// # Errors
    ///
    /// [`Refused::Rejected`] for every refusal; [`Refused::Internal`] when
    /// Core cannot do its part, in which case nothing was changed.
    pub fn complete(
        &self,
        arrival: &Arrival<'_>,
        request: &CompleteRequest,
    ) -> Result<CompleteResponse, Refused> {
        const ROUTE: &str = "pair-complete";
        let now = (self.clock)();
        // Single use: whatever happens next, this attempt is gone.
        let attempt = {
            let mut attempts = self.attempts()?;
            attempts.retain(|a| a.expires_at > now || a.pairing_id == request.pairing_id.0);
            attempts
                .iter()
                .position(|a| a.pairing_id == request.pairing_id.0)
                .map(|index| attempts.swap_remove(index))
        };

        let mut database = self.database()?;
        let transaction = database
            .pairing_transaction()
            .map_err(|error| internal(ROUTE, "transaction", &error))?;
        transaction
            .disarm_if_expired(now)
            .map_err(|error| internal(ROUTE, "expire", &error))?;
        let row = transaction
            .row()
            .map_err(|error| internal(ROUTE, "read", &error))?;
        let armed = row.armed.filter(|armed| {
            attempt
                .as_ref()
                .is_some_and(|a| a.arming_id == armed.arming_id)
        });

        // The first reason that applies, in a fixed order.
        let mut reason = match (&attempt, &armed) {
            (None, _) => Some(Reason::UnknownAttempt),
            (Some(_), None) => Some(Reason::ArmingGone),
            (Some(a), Some(_)) if a.connection != arrival.connection => {
                Some(Reason::OtherConnection)
            }
            (Some(a), Some(_)) if a.expires_at <= now => Some(Reason::AttemptExpired),
            _ if arrival.exporter.is_none() => Some(Reason::NoBinding),
            _ => None,
        };
        let secret = match (&armed, reason) {
            (Some(armed), None) => {
                let opened = self.open(armed);
                if opened.is_none() {
                    tracing::error!(
                        event = "pairing_secret_unsealable",
                        component = "atrium-core",
                        "the armed pairing secret does not open with this server's \
                         secrets.key; nothing can pair until it is re-armed with \
                         `atriumctl pair`"
                    );
                    reason = Some(Reason::Unsealable);
                }
                opened
            }
            _ => None,
        };

        // Always compute and compare, over placeholders if need be.
        let placeholder = PairingSecret::from_bytes([0; 16]);
        let inputs = TranscriptInputs {
            profile_hash: transcript::profile_hash(
                armed
                    .as_ref()
                    .map_or(NATIVE_PROFILE, |a| a.profile.as_str()),
            ),
            server_id: *self.facts.server_id.as_bytes(),
            spki: *self.facts.spki.as_bytes(),
            channel_binding: arrival.exporter.copied().unwrap_or([0; 32]),
            client_nonce: attempt.as_ref().map_or([0; 32], |a| a.client_nonce),
            server_nonce: attempt.as_ref().map_or([0; 32], |a| a.server_nonce),
            device_hash: attempt
                .as_ref()
                .map_or([0; 32], |a| transcript::device_hash(&a.device)),
        };
        let t = Zeroizing::new(transcript::transcript(&inputs));
        let key = transcript::pairing_key(
            secret.as_ref().unwrap_or(&placeholder),
            &inputs.server_nonce,
            &inputs.server_id,
        );
        let expected = Zeroizing::new(transcript::client_proof(&key, &t));
        let matches = transcript::proofs_match(&expected, &request.proof_c.0);
        if reason.is_none() && !matches {
            reason = Some(Reason::ProofMismatch);
        }

        if let Some(reason) = reason {
            // Only an attempt against the live arming counts against it.
            let counted =
                armed.is_some() && !matches!(reason, Reason::Unsealable | Reason::NoBinding);
            if counted {
                transaction
                    .record_failure()
                    .map_err(|error| internal(ROUTE, "record", &error))?;
            }
            // The count, and any expiry found above, are kept.
            transaction
                .commit()
                .map_err(|error| internal(ROUTE, "commit", &error))?;
            return Err(rejected(ROUTE, reason));
        }
        let Some(attempt) = attempt else {
            return Err(rejected(ROUTE, Reason::UnknownAttempt));
        };

        // Success: the device and its token, in the same transaction that
        // consumes the secret.
        let device_id = Id(random::<16>().map_err(|error| internal(ROUTE, "random", &error))?);
        let token = DeviceToken::from_bytes(
            random::<32>().map_err(|error| internal(ROUTE, "random", &error))?,
        );
        let digest = token.digest();
        let device_hex = device_id.to_hex();
        let first = transaction
            .consume(
                &NewDevice {
                    device_id: &device_hex,
                    name: attempt.device.name().as_str(),
                    platform: attempt.device.platform().as_str(),
                    token_digest: digest.as_bytes(),
                },
                now,
            )
            .map_err(|error| internal(ROUTE, "consume", &error))?;
        transaction
            .commit()
            .map_err(|error| internal(ROUTE, "commit", &error))?;
        drop(database);
        // The arming is gone; any other attempt for it could only fail.
        self.forget_all();

        let proof_s = transcript::server_proof(&key, &t);
        tracing::info!(
            event = "device_paired",
            component = "atrium-core",
            device_id = %device_hex,
            platform = attempt.device.platform().as_str(),
            first_claim = first,
            "a device was paired"
        );
        Ok(CompleteResponse {
            proof_s: Bytes(proof_s),
            device_id,
            device_token: token.encode().to_string(),
            role: "owner".to_owned(),
            server: ServerRef {
                id: Id(*self.facts.server_id.as_bytes()),
                name: self.facts.name.clone(),
            },
        })
    }

    fn open(&self, armed: &ArmedRow) -> Option<PairingSecret> {
        let context = seal::Context {
            server_id: self.facts.server_id.as_bytes(),
            arming_id: &armed.arming_id,
            profile: &armed.profile,
            armed_at: armed.armed_at,
            expires_at: armed.expires_at,
        };
        seal::open(&self.secrets, &context, &armed.nonce, &armed.ciphertext)
    }

    fn forget_all(&self) {
        if let Ok(mut attempts) = self.attempts.lock() {
            attempts.clear();
        }
    }

    /// Attempts in progress, for tests of the bound.
    #[cfg(test)]
    pub(crate) fn attempts_in_progress(&self) -> usize {
        self.attempts.lock().map(|a| a.len()).unwrap_or(0)
    }
}

/// A freshly armed secret, for `atriumctl pair` to show once.
pub struct NewlyArmed {
    /// The secret. Shown once, then dropped (and zeroed).
    pub secret: PairingSecret,
    /// When it stops working.
    pub expires_at: OffsetDateTime,
    /// What arming replaced.
    pub outcome: crate::db::pairing::Armed,
}

impl std::fmt::Debug for NewlyArmed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NewlyArmed")
            .field("expires_at", &self.expires_at)
            .field("outcome", &self.outcome)
            .finish_non_exhaustive()
    }
}

/// Why arming failed. Nothing was armed.
#[derive(Debug)]
pub enum ArmError {
    /// The OS random source failed.
    Random(getrandom::Error),
    /// The database refused.
    Database(rusqlite::Error),
}

impl std::fmt::Display for ArmError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Random(error) => write!(formatter, "no randomness available: {error}"),
            Self::Database(error) => write!(formatter, "the state database refused: {error}"),
        }
    }
}

impl std::error::Error for ArmError {}

/// Arms pairing for the native profile, replacing any armed secret:
/// `atriumctl pair`. One transaction; on any error
/// nothing is armed and no secret exists anywhere.
///
/// # Errors
///
/// [`ArmError`].
pub fn arm(
    database: &mut Database,
    secrets: &SecretsKey,
    server_id: &ServerId,
    now: OffsetDateTime,
) -> Result<NewlyArmed, ArmError> {
    // Whole seconds, so the sealed times and the stored times are equal.
    let armed_at = now.replace_nanosecond(0).unwrap_or(now);
    let expires_at = armed_at + SECRET_LIFETIME;
    let secret = PairingSecret::from_bytes(random::<16>().map_err(ArmError::Random)?);
    let arming_id = random::<16>().map_err(ArmError::Random)?;
    let context = seal::Context {
        server_id: server_id.as_bytes(),
        arming_id: &arming_id,
        profile: NATIVE_PROFILE,
        armed_at,
        expires_at,
    };
    let sealed = seal::seal(secrets, &context, &secret).map_err(ArmError::Random)?;
    let row = ArmedRow {
        arming_id,
        profile: NATIVE_PROFILE.to_owned(),
        nonce: sealed.nonce,
        ciphertext: sealed.ciphertext,
        armed_at,
        expires_at,
    };
    let outcome = database
        .arm_pairing(&row, Actor::Console, armed_at)
        .map_err(ArmError::Database)?;
    Ok(NewlyArmed {
        secret,
        expires_at,
        outcome,
    })
}

#[cfg(test)]
mod tests;
