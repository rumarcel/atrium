//! The server's TLS certificate: public, disposable, reissued from the
//! persistent key.
//!
//! A certificate is a statement *about* the identity key — which names and
//! addresses it answers to, and until when. It is not the identity. Clients
//! pin `SHA-256(SubjectPublicKeyInfo)`, which is a property of the key, so a
//! new certificate for the same key — new serial, new validity, new address
//! list — is invisible to a paired client (`docs/M1-IMPLEMENTATION-PLAN.md`
//! section 5.3). That is what lets this module reissue freely and why it never
//! touches the key: it has no way to write one.
//!
//! The certificate lives at `/var/lib/atrium/tls.crt` (PEM, `0644`) with its
//! record at `/var/lib/atrium/identity-state.json` (`0600`). The certificate
//! itself is the authority: the record must agree with it, and when it does
//! not, the certificate is reissued rather than the record trusted.
//!
//! Nothing here consults mDNS, a hostname, or anything a peer said. The
//! subject names are derived from `server_id` and the machine's own interface
//! addresses, and nothing about them affects trust — trust is the pin.

use std::collections::BTreeSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyUsagePurpose,
    SanType, SerialNumber,
};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};

use crate::fsio::{self, ReadError, Spec};
use crate::identity::{format_time, Identity, ServerId, SpkiPin};
use crate::layout::{Layout, CERTIFICATE_FILE, CERTIFICATE_STATE_FILE};

/// Validity of a certificate, inside the 398-day ceiling browsers enforce.
pub const VALIDITY: Duration = Duration::days(397);
/// Reissue when this close to expiry.
pub const REISSUE_WINDOW: Duration = Duration::days(30);
/// `notBefore` is backdated this much, so a client whose clock is slightly
/// behind does not see a certificate from the future.
pub const BACKDATE: Duration = Duration::hours(1);

/// `identity-state.json` schema version.
const STATE_FORMAT: u32 = 1;
/// Upper bounds on what is read back.
const CERTIFICATE_LIMIT: u64 = 64 * 1024;
const STATE_LIMIT: u64 = 64 * 1024;

/// The machine's own non-loopback addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AddressSet(BTreeSet<IpAddr>);

impl AddressSet {
    /// Builds a set, dropping loopback, unspecified and multicast addresses —
    /// the first two are always present by other means and the last is never
    /// an address a server answers on.
    pub fn new(addresses: impl IntoIterator<Item = IpAddr>) -> Self {
        Self(
            addresses
                .into_iter()
                .filter(|address| {
                    !address.is_loopback() && !address.is_unspecified() && !address.is_multicast()
                })
                .collect(),
        )
    }

    /// Every address on every interface right now.
    ///
    /// # Errors
    ///
    /// When the kernel's interface list cannot be read.
    pub fn current() -> std::io::Result<Self> {
        let interfaces = nix::ifaddrs::getifaddrs().map_err(std::io::Error::from)?;
        let mut addresses = Vec::new();
        for interface in interfaces {
            let Some(address) = interface.address else {
                continue;
            };
            if let Some(v4) = address.as_sockaddr_in() {
                addresses.push(IpAddr::V4(v4.ip()));
            } else if let Some(v6) = address.as_sockaddr_in6() {
                addresses.push(IpAddr::V6(v6.ip()));
            }
        }
        Ok(Self::new(addresses))
    }

    /// The addresses, in order.
    pub fn iter(&self) -> impl Iterator<Item = &IpAddr> {
        self.0.iter()
    }

    /// Number of addresses.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// True when the machine has no non-loopback address at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// The names and addresses a certificate covers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectNames {
    /// DNS names.
    pub dns: BTreeSet<String>,
    /// IP addresses.
    pub ips: BTreeSet<IpAddr>,
}

impl SubjectNames {
    /// What a certificate for this server should name, given its addresses:
    /// `atrium-<short_id>.local`, `localhost`, both loopbacks, and every
    /// current address. Nothing assumes a single address.
    #[must_use]
    pub fn for_server(server_id: &ServerId, addresses: &AddressSet) -> Self {
        let dns = [
            format!("atrium-{}.local", server_id.short_id()),
            "localhost".to_owned(),
        ]
        .into_iter()
        .collect();
        let mut ips: BTreeSet<IpAddr> = [
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
        ]
        .into_iter()
        .collect();
        ips.extend(addresses.iter().copied());
        Self { dns, ips }
    }
}

/// `identity-state.json` on disk.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StateFile {
    format: u32,
    serial: String,
    issued_at: String,
    not_before: String,
    not_after: String,
    spki_sha256: String,
    dns: Vec<String>,
    ips: Vec<String>,
}

/// What a certificate says, as recorded or as read from the certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateRecord {
    /// Serial number, lowercase hex.
    pub serial: String,
    /// Start of validity.
    pub not_before: OffsetDateTime,
    /// End of validity.
    pub not_after: OffsetDateTime,
    /// Pin of the key it certifies.
    pub spki: SpkiPin,
    /// Names and addresses covered.
    pub names: SubjectNames,
}

impl CertificateRecord {
    fn to_json(&self, issued_at: OffsetDateTime) -> Vec<u8> {
        let file = StateFile {
            format: STATE_FORMAT,
            serial: self.serial.clone(),
            issued_at: format_time(issued_at),
            not_before: format_time(self.not_before),
            not_after: format_time(self.not_after),
            spki_sha256: self.spki.to_string(),
            dns: self.names.dns.iter().cloned().collect(),
            ips: self.names.ips.iter().map(ToString::to_string).collect(),
        };
        let mut json = serde_json::to_vec_pretty(&file).expect("a flat record serialises");
        json.push(b'\n');
        json
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        let file: StateFile = serde_json::from_slice(bytes).ok()?;
        if file.format != STATE_FORMAT {
            return None;
        }
        OffsetDateTime::parse(&file.issued_at, &Rfc3339).ok()?;
        let mut ips = BTreeSet::new();
        for ip in &file.ips {
            ips.insert(ip.parse().ok()?);
        }
        Some(Self {
            serial: file.serial,
            not_before: OffsetDateTime::parse(&file.not_before, &Rfc3339).ok()?,
            not_after: OffsetDateTime::parse(&file.not_after, &Rfc3339).ok()?,
            spki: SpkiPin::parse(&file.spki_sha256)?,
            names: SubjectNames {
                dns: file.dns.into_iter().collect(),
                ips,
            },
        })
    }
}

/// Why a certificate could not be produced.
#[derive(Debug)]
pub enum CertificateError {
    /// Randomness for the serial number was unavailable.
    Randomness,
    /// A name could not be encoded, or signing failed.
    Build(&'static str),
    /// Writing the certificate or its record failed.
    Write(std::io::Error),
}

impl fmt::Display for CertificateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Randomness => formatter.write_str("no randomness for a serial number"),
            Self::Build(what) => write!(formatter, "the certificate could not be built: {what}"),
            Self::Write(error) => {
                write!(formatter, "the certificate could not be written: {error}")
            }
        }
    }
}

impl std::error::Error for CertificateError {}

/// A freshly issued certificate, not yet on disk.
#[derive(Debug)]
pub struct Issued {
    /// PEM encoding.
    pub pem: String,
    /// What it says.
    pub record: CertificateRecord,
    /// When it was made.
    pub issued_at: OffsetDateTime,
}

/// Issues a self-signed leaf for `identity`'s key covering `names`.
///
/// # Errors
///
/// [`CertificateError::Randomness`] or [`CertificateError::Build`].
pub fn issue(
    identity: &Identity,
    names: &SubjectNames,
    now: OffsetDateTime,
) -> Result<Issued, CertificateError> {
    let now = now.replace_nanosecond(0).unwrap_or(now);
    let not_before = now - BACKDATE;
    let not_after = not_before + VALIDITY;

    // Sixteen random bytes with the top bit clear and the next one set: a
    // positive integer whose DER encoding is exactly these sixteen bytes, so
    // the serial read back from the certificate is byte-identical.
    let mut serial = [0_u8; 16];
    getrandom::fill(&mut serial).map_err(|_| CertificateError::Randomness)?;
    serial[0] = (serial[0] & 0x7f) | 0x40;

    let mut params = CertificateParams::default();
    let mut subject = DistinguishedName::new();
    subject.push(
        DnType::CommonName,
        format!("Atrium {}", identity.server_id().short_id()),
    );
    params.distinguished_name = subject;
    params.not_before = not_before;
    params.not_after = not_after;
    params.serial_number = Some(SerialNumber::from_slice(&serial));
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    for dns in &names.dns {
        let name = dns
            .clone()
            .try_into()
            .map_err(|_| CertificateError::Build("a DNS name is not IA5"))?;
        params.subject_alt_names.push(SanType::DnsName(name));
    }
    for ip in &names.ips {
        params.subject_alt_names.push(SanType::IpAddress(*ip));
    }

    let certificate = params
        .self_signed(identity.key.pair())
        .map_err(|_| CertificateError::Build("signing failed"))?;

    Ok(Issued {
        pem: certificate.pem(),
        record: CertificateRecord {
            serial: hex::encode(serial),
            not_before,
            not_after,
            spki: identity.pin(),
            names: names.clone(),
        },
        issued_at: now,
    })
}

/// Writes a certificate and then its record, each atomically.
///
/// The order matters: a crash between the two leaves a record that does not
/// match the certificate, which the next start notices and repairs by
/// reissuing. It never leaves a record describing a certificate that does
/// not exist.
///
/// # Errors
///
/// [`CertificateError::Write`]; the previous files stay in place.
pub fn write(layout: &Layout, issued: &Issued) -> Result<(), CertificateError> {
    let dir = layout.state_dir();
    fsio::replace(
        dir,
        CERTIFICATE_FILE,
        issued.pem.as_bytes(),
        Spec::mode(0o644),
    )
    .map_err(CertificateError::Write)?;
    fsio::replace(
        dir,
        CERTIFICATE_STATE_FILE,
        &issued.record.to_json(issued.issued_at),
        Spec::mode(0o600),
    )
    .map_err(CertificateError::Write)
}

/// Why the certificate on disk cannot be served as it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReissueCause {
    /// There is no certificate.
    Missing,
    /// The file is not a parseable certificate, or cannot be read.
    Malformed,
    /// It certifies a different key.
    WrongKey,
    /// Its record is missing or describes a different certificate.
    RecordStale,
    /// It has expired.
    Expired,
    /// It is not valid yet — the clock moved backwards.
    NotYetValid,
    /// It expires within [`REISSUE_WINDOW`].
    ExpiresSoon,
    /// The machine's addresses are not the ones it names.
    AddressesChanged,
}

impl ReissueCause {
    /// Stable, loggable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Malformed => "malformed",
            Self::WrongKey => "wrong_key",
            Self::RecordStale => "record_stale",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::ExpiresSoon => "expires_soon",
            Self::AddressesChanged => "addresses_changed",
        }
    }

    /// True when the existing certificate can still be served if a reissue
    /// fails. Everything else leaves nothing usable.
    #[must_use]
    pub fn old_one_still_usable(self) -> bool {
        matches!(
            self,
            Self::ExpiresSoon | Self::AddressesChanged | Self::RecordStale
        )
    }
}

/// Reads the certificate on disk and describes it, or says why it is unusable.
fn read_current(layout: &Layout, identity: &Identity) -> Result<CertificateRecord, ReissueCause> {
    let pem = match fsio::read_regular(&layout.state_file(CERTIFICATE_FILE), CERTIFICATE_LIMIT) {
        Ok(pem) => pem,
        Err(ReadError::Missing) => return Err(ReissueCause::Missing),
        Err(_) => return Err(ReissueCause::Malformed),
    };
    let observed = describe_pem(&pem).ok_or(ReissueCause::Malformed)?;
    if observed.spki != identity.pin() {
        return Err(ReissueCause::WrongKey);
    }
    Ok(observed)
}

/// Parses a PEM certificate into what it says.
#[must_use]
pub fn describe_pem(pem: &[u8]) -> Option<CertificateRecord> {
    let (rest, block) = x509_parser::pem::parse_x509_pem(pem).ok()?;
    if block.label != "CERTIFICATE" || !rest.iter().all(u8::is_ascii_whitespace) {
        return None;
    }
    let certificate = block.parse_x509().ok()?;

    let mut dns = BTreeSet::new();
    let mut ips = BTreeSet::new();
    if let Ok(Some(extension)) = certificate.subject_alternative_name() {
        for name in &extension.value.general_names {
            match name {
                x509_parser::extensions::GeneralName::DNSName(value) => {
                    dns.insert((*value).to_owned());
                }
                x509_parser::extensions::GeneralName::IPAddress(bytes) => {
                    let ip = match bytes.len() {
                        4 => IpAddr::from(<[u8; 4]>::try_from(*bytes).ok()?),
                        16 => IpAddr::from(<[u8; 16]>::try_from(*bytes).ok()?),
                        _ => return None,
                    };
                    ips.insert(ip);
                }
                _ => return None,
            }
        }
    }

    let validity = certificate.validity();
    Some(CertificateRecord {
        serial: hex::encode(certificate.raw_serial()),
        not_before: OffsetDateTime::from_unix_timestamp(validity.not_before.timestamp()).ok()?,
        not_after: OffsetDateTime::from_unix_timestamp(validity.not_after.timestamp()).ok()?,
        spki: SpkiPin::of(certificate.public_key().raw),
        names: SubjectNames { dns, ips },
    })
}

/// Decides whether `current` can be served for `wanted` at `now`.
#[must_use]
pub fn assess(
    current: &CertificateRecord,
    recorded: Option<&CertificateRecord>,
    wanted: &SubjectNames,
    now: OffsetDateTime,
) -> Option<ReissueCause> {
    if now >= current.not_after {
        return Some(ReissueCause::Expired);
    }
    if now < current.not_before {
        return Some(ReissueCause::NotYetValid);
    }
    if current.not_after - now < REISSUE_WINDOW {
        return Some(ReissueCause::ExpiresSoon);
    }
    if &current.names != wanted {
        return Some(ReissueCause::AddressesChanged);
    }
    if recorded != Some(current) {
        return Some(ReissueCause::RecordStale);
    }
    None
}

/// What [`ensure`] did.
#[derive(Debug)]
pub enum Outcome {
    /// The certificate on disk was already right.
    Kept(CertificateRecord),
    /// A new certificate, same key, is on disk.
    Reissued {
        /// What it says.
        record: CertificateRecord,
        /// Why the old one was replaced.
        cause: ReissueCause,
    },
    /// A reissue was needed and failed, and the old certificate is still
    /// valid, so it stays in service. Retried on the next check.
    KeptAfterFailure {
        /// The certificate still being served.
        record: CertificateRecord,
        /// Why a reissue was attempted.
        cause: ReissueCause,
        /// Why it failed.
        error: String,
    },
}

/// Makes sure a usable certificate for `identity` covering `addresses` is on
/// disk, reissuing from the existing key when it is not.
///
/// # Errors
///
/// Only when no usable certificate exists afterwards. A failed reissue with a
/// still-valid old certificate is [`Outcome::KeptAfterFailure`], not an error:
/// plan section 19 says to keep serving the old one.
pub fn ensure(
    layout: &Layout,
    identity: &Identity,
    addresses: &AddressSet,
    now: OffsetDateTime,
) -> Result<Outcome, CertificateError> {
    let wanted = SubjectNames::for_server(&identity.server_id(), addresses);
    let recorded = fsio::read_regular(&layout.state_file(CERTIFICATE_STATE_FILE), STATE_LIMIT)
        .ok()
        .and_then(|bytes| CertificateRecord::parse(&bytes));

    let (cause, existing) = match read_current(layout, identity) {
        Ok(current) => match assess(&current, recorded.as_ref(), &wanted, now) {
            None => return Ok(Outcome::Kept(current)),
            Some(cause) => (cause, Some(current)),
        },
        Err(cause) => (cause, None),
    };

    let attempt = issue(identity, &wanted, now).and_then(|issued| {
        write(layout, &issued)?;
        Ok(issued.record)
    });
    match (attempt, existing) {
        (Ok(record), _) => Ok(Outcome::Reissued { record, cause }),
        (Err(error), Some(record)) if cause.old_one_still_usable() => {
            Ok(Outcome::KeptAfterFailure {
                record,
                cause,
                error: error.to_string(),
            })
        }
        (Err(error), _) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{DeviceKey, IdentityRecord};

    fn identity() -> Identity {
        let key = DeviceKey::generate().expect("keygen");
        Identity {
            record: IdentityRecord {
                server_id: ServerId::generate().expect("randomness"),
                created_at: OffsetDateTime::now_utc(),
                spki: key.pin(),
            },
            key,
        }
    }

    fn addresses(list: &[&str]) -> AddressSet {
        AddressSet::new(list.iter().map(|a| a.parse().expect("address")))
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_790_000_000).expect("time")
    }

    #[test]
    fn subject_names_cover_every_address_and_both_loopbacks() {
        let identity = identity();
        let names = SubjectNames::for_server(
            &identity.server_id(),
            &addresses(&["192.0.2.10", "198.51.100.7", "2001:db8::1", "127.0.0.1"]),
        );
        let short = identity.server_id().short_id();
        assert!(names.dns.contains(&format!("atrium-{short}.local")));
        assert!(names.dns.contains("localhost"));
        assert_eq!(names.dns.len(), 2);
        for ip in [
            "127.0.0.1",
            "::1",
            "192.0.2.10",
            "198.51.100.7",
            "2001:db8::1",
        ] {
            assert!(names.ips.contains(&ip.parse().expect("ip")), "missing {ip}");
        }
        assert_eq!(names.ips.len(), 5);
    }

    #[test]
    fn an_issued_certificate_says_what_the_plan_says() {
        let identity = identity();
        let names = SubjectNames::for_server(&identity.server_id(), &addresses(&["192.0.2.10"]));
        let issued = issue(&identity, &names, now()).expect("issued");

        let (_, block) = x509_parser::pem::parse_x509_pem(issued.pem.as_bytes()).expect("pem");
        let certificate = block.parse_x509().expect("der");

        let common_name = certificate
            .subject()
            .iter_common_name()
            .next()
            .and_then(|cn| cn.as_str().ok())
            .expect("CN");
        assert_eq!(
            common_name,
            format!("Atrium {}", identity.server_id().short_id())
        );

        let usage = certificate.key_usage().expect("parse").expect("present");
        assert!(usage.value.digital_signature());
        let extended = certificate
            .extended_key_usage()
            .expect("parse")
            .expect("present");
        assert!(extended.value.server_auth);

        let described = describe_pem(issued.pem.as_bytes()).expect("describable");
        assert_eq!(described, issued.record);
        assert_eq!(described.not_after - described.not_before, VALIDITY);
        assert_eq!(described.spki, identity.pin());
    }

    #[test]
    fn the_certificate_spki_is_the_key_spki_byte_for_byte() {
        let identity = identity();
        let names = SubjectNames::for_server(&identity.server_id(), &AddressSet::default());
        let issued = issue(&identity, &names, now()).expect("issued");
        let (_, block) = x509_parser::pem::parse_x509_pem(issued.pem.as_bytes()).expect("pem");
        let certificate = block.parse_x509().expect("der");
        assert_eq!(certificate.public_key().raw, identity.key.spki_der());
    }

    #[test]
    fn spki_survives_certificate_reissue() {
        let identity = identity();
        let before = issue(
            &identity,
            &SubjectNames::for_server(&identity.server_id(), &addresses(&["192.0.2.10"])),
            now(),
        )
        .expect("first");
        let after = issue(
            &identity,
            &SubjectNames::for_server(&identity.server_id(), &addresses(&["203.0.113.5"])),
            now() + Duration::minutes(5),
        )
        .expect("second");

        let pin_before = describe_pem(before.pem.as_bytes()).expect("before").spki;
        let pin_after = describe_pem(after.pem.as_bytes()).expect("after").spki;
        assert_eq!(
            pin_before, pin_after,
            "the pin is the key's, not the certificate's"
        );
        assert_eq!(pin_before, identity.pin());
    }

    #[test]
    fn reissue_changes_serial_and_sans() {
        let identity = identity();
        let first = issue(
            &identity,
            &SubjectNames::for_server(&identity.server_id(), &addresses(&["192.0.2.10"])),
            now(),
        )
        .expect("first");
        let second = issue(
            &identity,
            &SubjectNames::for_server(&identity.server_id(), &addresses(&["203.0.113.5"])),
            now(),
        )
        .expect("second");
        assert_ne!(first.record.serial, second.record.serial);
        assert_ne!(first.record.names, second.record.names);
        assert_ne!(first.pem, second.pem);
    }

    #[test]
    fn assessment_covers_every_reason_to_reissue() {
        let identity = identity();
        let wanted = SubjectNames::for_server(&identity.server_id(), &addresses(&["192.0.2.10"]));
        let current = issue(&identity, &wanted, now()).expect("issued").record;
        let recorded = Some(&current);

        assert_eq!(assess(&current, recorded, &wanted, now()), None);
        assert_eq!(
            assess(&current, None, &wanted, now()),
            Some(ReissueCause::RecordStale)
        );
        let moved = SubjectNames::for_server(&identity.server_id(), &addresses(&["192.0.2.11"]));
        assert_eq!(
            assess(&current, recorded, &moved, now()),
            Some(ReissueCause::AddressesChanged)
        );
        assert_eq!(
            assess(
                &current,
                recorded,
                &wanted,
                current.not_after - Duration::days(29)
            ),
            Some(ReissueCause::ExpiresSoon)
        );
        assert_eq!(
            assess(
                &current,
                recorded,
                &wanted,
                current.not_after - Duration::days(31)
            ),
            None
        );
        assert_eq!(
            assess(&current, recorded, &wanted, current.not_after),
            Some(ReissueCause::Expired)
        );
        assert_eq!(
            assess(
                &current,
                recorded,
                &wanted,
                current.not_before - Duration::seconds(1)
            ),
            Some(ReissueCause::NotYetValid)
        );
    }

    #[test]
    fn garbage_is_not_a_certificate() {
        assert!(describe_pem(b"").is_none());
        assert!(
            describe_pem(b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n")
                .is_none()
        );
        let identity = identity();
        let pem = identity.key.to_pkcs8();
        assert!(describe_pem(pem.expose()).is_none());
    }

    #[test]
    fn the_record_round_trips() {
        let identity = identity();
        let names = SubjectNames::for_server(&identity.server_id(), &addresses(&["2001:db8::5"]));
        let issued = issue(&identity, &names, now()).expect("issued");
        let json = issued.record.to_json(issued.issued_at);
        assert_eq!(CertificateRecord::parse(&json), Some(issued.record));
        assert!(CertificateRecord::parse(b"{}").is_none());
    }
}
