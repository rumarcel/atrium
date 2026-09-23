//! Agent's own journal: one JSON line per connection, whatever the outcome.
//!
//! `<state dir>/journal/<YYYY-MM-DD>-<first seq>.jsonl`, `0600`, in a `0700`
//! directory inside Agent's `0700` state directory. Only Agent's uid can
//! reach it. In production that is root, so a compromised Core, running as
//! `atrium`, cannot read, edit, truncate or delete a line. That independence
//! is the point. Core keeps its own audit log (plan section 15), and a
//! compromised Core can rewrite *that*. It cannot rewrite this.
//!
//! Properties:
//!
//! - **Agent's sequence and Agent's clock.** `seq` increases by one per
//!   line, across restarts and file rotation, and is never reused: a number
//!   spent on a failed write is not handed out again. `ts` is Agent's clock.
//! - **Nothing unvalidated.** A line is serialized from typed values:
//!   integers from the kernel, closed enums, and the protocol's validated
//!   newtypes. No raw frame byte and no free-form string is ever written.
//! - **Flushed per line.** Each line is written with one `write` call and
//!   `fdatasync`ed before the result it records is sent.
//! - **Checked before use.** Before opening anything, the state directory and
//!   the journal directory must be real directories, owned by Agent's uid,
//!   with no group or other access. Files are opened with `O_NOFOLLOW` and
//!   checked the same way after opening. Anything else makes the journal
//!   unavailable, and an unavailable journal means Agent performs no
//!   operation (see [`crate::serve`]).
//! - **Bounded.** A new file starts on a new UTC day or at 8 MiB; the newest
//!   five are kept (plan section 3.6).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use atrium_protocol::messages::AgentOp;
use atrium_protocol::values::{RequestId, Timestamp, Version};
use serde::Serialize;

use crate::clock::{self, Day};

/// Subdirectory of the state directory.
pub const JOURNAL_DIR: &str = "journal";
/// A file is closed for appending once it reaches this size.
pub const FILE_LIMIT: u64 = 8 * 1024 * 1024;
/// How many files are kept.
pub const KEEP_FILES: usize = 5;
/// The tail of the newest file read at startup to recover `seq`.
const RECOVERY_TAIL: u64 = 256 * 1024;

/// The peer, as the kernel reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Peer {
    /// Real uid of the connecting process.
    pub uid: u32,
    /// Real gid of the connecting process.
    pub gid: u32,
    /// Its pid, when the kernel reported one.
    pub pid: Option<i32>,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The operation ran and its result was sent.
    Ok,
    /// Agent refused before performing anything.
    Refused,
}

/// Why a connection was refused. A closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// The peer's uid is not Core's.
    PeerUidRejected,
    /// The kernel did not report the peer's credentials.
    PeerCredentialsUnavailable,
    /// Too many connections too quickly; see `suppressed`.
    RateLimited,
    /// The peer closed without sending a frame.
    NoRequest,
    /// A zero length prefix.
    EmptyFrame,
    /// A length prefix over the limit; the body was not read.
    FrameTooLarge,
    /// The peer closed partway through a frame.
    TruncatedFrame,
    /// Not a frame this protocol defines.
    MalformedFrame,
    /// `request_id` was not 1 to 64 lowercase hex characters. It is not
    /// written here; `seq` is the only correlation for this line.
    InvalidRequestId,
    /// `core_version` was not a version.
    InvalidCoreVersion,
    /// `hello.protocol` was not Agent's.
    ProtocolVersionMismatch,
    /// A valid frame in the wrong place.
    UnexpectedFrame,
    /// A `call` naming an undefined operation.
    UnknownOperation,
    /// The exchange did not finish within the deadline.
    TimedOut,
    /// The peer went away before Agent could answer the handshake.
    PeerDisconnected,
}

/// One line, before Agent numbers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Record {
    /// Agent's clock.
    pub ts: Timestamp,
    /// The peer; `null` for a rate-limit summary.
    pub peer: Option<Peer>,
    /// The operation ran.
    pub accepted: bool,
    /// Core's correlation id, only once validated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    /// Core's version, only once validated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub core_version: Option<Version>,
    /// The protocol version a mismatched hello declared.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub found_protocol: Option<u32>,
    /// The operation, once parsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op: Option<AgentOp>,
    /// What happened.
    pub outcome: Outcome,
    /// Why, when refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<Reason>,
    /// Connections closed unjournaled by the rate limit since the last line.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppressed: Option<u64>,
    /// Wall time of the exchange.
    pub duration_ms: u64,
}

impl Record {
    /// A refusal from `peer` for `reason`, with nothing else known yet.
    #[must_use]
    pub fn refused(ts: Timestamp, peer: Option<Peer>, reason: Reason) -> Self {
        Self {
            ts,
            peer,
            accepted: false,
            request_id: None,
            core_version: None,
            found_protocol: None,
            op: None,
            outcome: Outcome::Refused,
            reason: Some(reason),
            suppressed: None,
            duration_ms: 0,
        }
    }
}

#[derive(Serialize)]
struct Line<'a> {
    seq: u64,
    #[serde(flatten)]
    record: &'a Record,
}

/// Why the journal could not be written.
#[derive(Debug)]
pub enum JournalError {
    /// A directory or file is not what it must be.
    Unprotected(PathBuf, &'static str),
    /// The filesystem refused.
    Io(std::io::Error),
}

impl std::fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unprotected(path, why) => write!(formatter, "{}: {why}", path.display()),
            Self::Io(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<std::io::Error> for JournalError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

struct Current {
    file: File,
    size: u64,
    day: Day,
}

/// The journal. Opened lazily and reopened after any failure, so a
/// directory that is repaired while Agent runs is picked up again.
pub struct Journal {
    state_dir: PathBuf,
    owner: u32,
    last_seq: u64,
    recovered: bool,
    current: Option<Current>,
    writable: bool,
}

impl Journal {
    /// A journal under `state_dir`, owned by `owner` (Agent's euid).
    #[must_use]
    pub fn new(state_dir: PathBuf, owner: u32) -> Self {
        Self {
            state_dir,
            owner,
            last_seq: 0,
            recovered: false,
            current: None,
            writable: false,
        }
    }

    /// The state directory in use.
    #[must_use]
    pub fn state_dir(&self) -> &Path {
        &self.state_dir
    }

    /// Whether the last open or append succeeded.
    #[must_use]
    pub fn writable(&self) -> bool {
        self.writable
    }

    /// The last sequence number handed out.
    #[must_use]
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }

    fn dir(&self) -> PathBuf {
        self.state_dir.join(JOURNAL_DIR)
    }

    /// Opens the journal now, so startup reports a problem at once.
    ///
    /// # Errors
    ///
    /// See [`Journal::append`].
    pub fn open(&mut self, now: SystemTime) -> Result<(), JournalError> {
        let result = self.ensure_open(clock::day(now), 0);
        self.writable = result.is_ok();
        if result.is_err() {
            self.current = None;
        }
        result
    }

    /// Appends one line and returns its sequence number.
    ///
    /// # Errors
    ///
    /// [`JournalError`] when the directories fail their checks or the write
    /// or flush fails. The journal is then marked unwritable and reopened
    /// from scratch on the next call.
    pub fn append(&mut self, record: &Record, now: SystemTime) -> Result<u64, JournalError> {
        let result = self.try_append(record, now);
        self.writable = result.is_ok();
        if result.is_err() {
            self.current = None;
        }
        result
    }

    fn try_append(&mut self, record: &Record, now: SystemTime) -> Result<u64, JournalError> {
        let today = clock::day(now);
        // Open (and, the first time, recover the sequence) before numbering:
        // the number must come after everything already on disk.
        self.ensure_open(today, 0)?;
        let seq = self.last_seq.saturating_add(1);
        let mut line = serde_json::to_vec(&Line { seq, record })
            .map_err(|error| JournalError::Io(std::io::Error::other(error)))?;
        line.push(b'\n');
        let length = u64::try_from(line.len()).unwrap_or(u64::MAX);
        // Rolls over if this line would not fit; the sequence is unaffected.
        self.ensure_open(today, length)?;
        // Spent from here on: a failure below may leave part of this line
        // behind, and the number must not be handed out a second time.
        self.last_seq = seq;
        let current = self
            .current
            .as_mut()
            .ok_or_else(|| JournalError::Io(std::io::Error::other("journal not open")))?;
        current.file.write_all(&line)?;
        current.file.sync_data()?;
        current.size = current.size.saturating_add(length);
        Ok(seq)
    }

    /// Makes sure a file is open that can take `incoming` more bytes today.
    fn ensure_open(&mut self, today: Day, incoming: u64) -> Result<(), JournalError> {
        // Re-checked on every append, not only on open: a directory whose
        // protection changed, or a file unlinked from under an open handle,
        // must stop the journal rather than let lines go where nobody will
        // find them.
        check_private_dir(&self.state_dir, self.owner)?;
        let dir = self.dir();
        if let Some(current) = &self.current {
            check_private_dir(&dir, self.owner)?;
            let linked = current.file.metadata()?.nlink() > 0;
            if linked && current.day == today && current.size.saturating_add(incoming) <= FILE_LIMIT
            {
                return Ok(());
            }
            self.current = None;
        }

        match std::fs::symlink_metadata(&dir) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
                sync_dir(&self.state_dir)?;
            }
            Err(error) => return Err(error.into()),
        }
        check_private_dir(&dir, self.owner)?;

        let files = list(&dir)?;
        if !self.recovered {
            self.last_seq = recover_last_seq(&dir, &files, self.owner)?.max(self.last_seq);
            self.recovered = true;
        }

        // Continue the newest file if it is today's and has room.
        if let Some(newest) = files.last() {
            if newest.day == today.iso() {
                let path = dir.join(&newest.name);
                let mut file = open_append(&path, self.owner, false)?;
                let mut size = file.metadata()?.len();
                if size.saturating_add(incoming + 1) <= FILE_LIMIT {
                    // Close off a line torn by a crash, so the next line
                    // starts on its own and every complete line parses.
                    if size > 0 && !ends_with_newline(&path, self.owner)? {
                        file.write_all(b"\n")?;
                        file.sync_data()?;
                        size += 1;
                    }
                    self.current = Some(Current {
                        file,
                        size,
                        day: today,
                    });
                    return Ok(());
                }
            }
        }

        let first = self.last_seq.saturating_add(1);
        let name = format!("{}-{first:020}.jsonl", today.iso());
        let file = open_append(&dir.join(&name), self.owner, true)?;
        sync_dir(&dir)?;
        self.current = Some(Current {
            file,
            size: 0,
            day: today,
        });
        prune(&dir)?;
        Ok(())
    }
}

/// A journal file's name, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    name: String,
    day: String,
    first_seq: u64,
}

/// `YYYY-MM-DD-<20 digits>.jsonl`; `None` for anything else.
fn parse_name(name: &str) -> Option<Entry> {
    let stem = name.strip_suffix(".jsonl")?;
    if stem.len() != 31 || stem.as_bytes()[10] != b'-' {
        return None;
    }
    let (day, seq) = (&stem[..10], &stem[11..]);
    let day_ok = day.bytes().enumerate().all(|(i, b)| match i {
        4 | 7 => b == b'-',
        _ => b.is_ascii_digit(),
    });
    if !day_ok || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(Entry {
        name: name.to_owned(),
        day: day.to_owned(),
        first_seq: seq.parse().ok()?,
    })
}

/// Journal files, oldest first by their first sequence number. Anything
/// that does not match the name pattern is left alone.
fn list(dir: &Path) -> Result<Vec<Entry>, JournalError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if let Some(parsed) = entry.file_name().to_str().and_then(parse_name) {
            entries.push(parsed);
        }
    }
    entries.sort_by_key(|entry| entry.first_seq);
    Ok(entries)
}

/// The highest `seq` on disk: from the newest file's last complete lines,
/// and at least that file's first number. A file is only ever created to
/// take its first line, so that number counts as spent even when the line
/// never landed, and a new file can never be given an existing name.
fn recover_last_seq(dir: &Path, files: &[Entry], owner: u32) -> Result<u64, JournalError> {
    #[derive(serde::Deserialize)]
    struct SeqOnly {
        seq: u64,
    }
    let Some(newest) = files.last() else {
        return Ok(0);
    };
    let mut highest = newest.first_seq;
    let mut file = open_read(&dir.join(&newest.name), owner)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(RECOVERY_TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut tail = Vec::new();
    file.take(RECOVERY_TAIL).read_to_end(&mut tail)?;
    for line in tail.split(|b| *b == b'\n') {
        if let Ok(parsed) = serde_json::from_slice::<SeqOnly>(line) {
            highest = highest.max(parsed.seq);
        }
    }
    // A file that does not end in a newline ends in a line torn by a crash.
    // That line took the next number, whether or not its digits landed.
    if tail.last().is_some_and(|b| *b != b'\n') {
        highest = highest.saturating_add(1);
    }
    Ok(highest)
}

fn ends_with_newline(path: &Path, owner: u32) -> Result<bool, JournalError> {
    let mut file = open_read(path, owner)?;
    file.seek(SeekFrom::End(-1))?;
    let mut last = [0_u8; 1];
    file.read_exact(&mut last)?;
    Ok(last[0] == b'\n')
}

/// Removes all but the newest [`KEEP_FILES`].
fn prune(dir: &Path) -> Result<(), JournalError> {
    let files = list(dir)?;
    let excess = files.len().saturating_sub(KEEP_FILES);
    for entry in &files[..excess] {
        std::fs::remove_file(dir.join(&entry.name))?;
    }
    if excess > 0 {
        sync_dir(dir)?;
    }
    Ok(())
}

fn nofollow() -> i32 {
    nix::fcntl::OFlag::O_NOFOLLOW.bits()
}

fn open_append(path: &Path, owner: u32, create: bool) -> Result<File, JournalError> {
    let file = OpenOptions::new()
        .append(true)
        .create_new(create)
        .mode(0o600)
        .custom_flags(nofollow())
        .open(path)?;
    check_private_file(&file, path, owner)?;
    Ok(file)
}

fn open_read(path: &Path, owner: u32) -> Result<File, JournalError> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nofollow())
        .open(path)?;
    check_private_file(&file, path, owner)?;
    Ok(file)
}

fn check_private_file(file: &File, path: &Path, owner: u32) -> Result<(), JournalError> {
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        return Err(JournalError::Unprotected(path.into(), "not a regular file"));
    }
    if metadata.uid() != owner {
        return Err(JournalError::Unprotected(path.into(), "not owned by Agent"));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(JournalError::Unprotected(
            path.into(),
            "accessible to group or other",
        ));
    }
    Ok(())
}

/// A real directory, owned by `owner`, with no group or other access.
///
/// # Errors
///
/// [`JournalError::Unprotected`] naming the property that failed.
pub fn check_private_dir(path: &Path, owner: u32) -> Result<(), JournalError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() {
        return Err(JournalError::Unprotected(
            path.into(),
            "not a directory (or a symbolic link)",
        ));
    }
    if metadata.uid() != owner {
        return Err(JournalError::Unprotected(path.into(), "not owned by Agent"));
    }
    if metadata.mode() & 0o077 != 0 {
        return Err(JournalError::Unprotected(
            path.into(),
            "accessible to group or other",
        ));
    }
    Ok(())
}

fn sync_dir(path: &Path) -> Result<(), JournalError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, UNIX_EPOCH};

    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("atrium-agent-journal-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir(&path).expect("dir");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).expect("chmod");
            Self(path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn euid() -> u32 {
        nix::unistd::geteuid().as_raw()
    }

    fn when(day: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1_790_000_000 + day * 86_400)
    }

    fn record(now: SystemTime) -> Record {
        let mut record = Record::refused(
            clock::timestamp(now),
            Some(Peer {
                uid: 1,
                gid: 2,
                pid: Some(3),
            }),
            Reason::MalformedFrame,
        );
        record.request_id = Some(RequestId::parse("ab").expect("id"));
        record
    }

    fn lines(dir: &Path) -> Vec<serde_json::Value> {
        let mut all = Vec::new();
        for entry in list(&dir.join(JOURNAL_DIR)).expect("list") {
            let text =
                std::fs::read_to_string(dir.join(JOURNAL_DIR).join(&entry.name)).expect("read");
            for line in text.lines() {
                all.push(serde_json::from_str(line).expect("each line is JSON"));
            }
        }
        all
    }

    #[test]
    fn lines_are_numbered_from_one_and_carry_only_typed_values() {
        let dir = Dir::new("numbered");
        let mut journal = Journal::new(dir.0.clone(), euid());
        let now = when(0);
        assert_eq!(journal.append(&record(now), now).expect("append"), 1);
        assert_eq!(journal.append(&record(now), now).expect("append"), 2);
        let written = lines(&dir.0);
        assert_eq!(written.len(), 2);
        assert_eq!(written[0]["seq"], 1);
        assert_eq!(written[1]["seq"], 2);
        assert_eq!(written[0]["reason"], "malformed_frame");
        assert_eq!(written[0]["request_id"], "ab");
        assert_eq!(written[0]["peer"]["uid"], 1);
        assert!(journal.writable());
    }

    #[test]
    fn modes_are_private() {
        let dir = Dir::new("modes");
        let mut journal = Journal::new(dir.0.clone(), euid());
        journal.append(&record(when(0)), when(0)).expect("append");
        let journal_dir = dir.0.join(JOURNAL_DIR);
        let mode = std::fs::metadata(&journal_dir).expect("stat").mode() & 0o777;
        assert_eq!(mode, 0o700);
        for entry in list(&journal_dir).expect("list") {
            let mode = std::fs::metadata(journal_dir.join(entry.name))
                .expect("stat")
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn sequence_survives_a_restart() {
        let dir = Dir::new("restart");
        let now = when(0);
        {
            let mut journal = Journal::new(dir.0.clone(), euid());
            for _ in 0..3 {
                journal.append(&record(now), now).expect("append");
            }
        }
        let mut journal = Journal::new(dir.0.clone(), euid());
        journal.open(now).expect("open");
        assert_eq!(journal.last_seq(), 3);
        assert_eq!(journal.append(&record(now), now).expect("append"), 4);
    }

    #[test]
    fn a_torn_last_line_does_not_reset_the_sequence() {
        let dir = Dir::new("torn");
        let now = when(0);
        {
            let mut journal = Journal::new(dir.0.clone(), euid());
            journal.append(&record(now), now).expect("append");
            journal.append(&record(now), now).expect("append");
        }
        let journal_dir = dir.0.join(JOURNAL_DIR);
        let entry = list(&journal_dir).expect("list").pop().expect("a file");
        let path = journal_dir.join(entry.name);
        let mut file = OpenOptions::new().append(true).open(&path).expect("open");
        file.write_all(b"{\"seq\":3,\"ts\":\"20").expect("tear");
        drop(file);

        let mut journal = Journal::new(dir.0.clone(), euid());
        assert_eq!(
            journal.append(&record(now), now).expect("append"),
            4,
            "the torn line spent 3"
        );
        let text = std::fs::read_to_string(&path).expect("read");
        let complete: Vec<&str> = text
            .lines()
            .filter(|line| serde_json::from_str::<serde_json::Value>(line).is_ok())
            .collect();
        assert_eq!(
            complete.len(),
            3,
            "the new line is not glued to the torn one"
        );
        assert!(complete[2].starts_with("{\"seq\":4,"));
    }

    #[test]
    fn a_new_day_starts_a_new_file_and_five_are_kept() {
        let dir = Dir::new("rotate");
        let mut journal = Journal::new(dir.0.clone(), euid());
        for day in 0..8 {
            journal
                .append(&record(when(day)), when(day))
                .expect("append");
        }
        let files = list(&dir.0.join(JOURNAL_DIR)).expect("list");
        assert_eq!(files.len(), KEEP_FILES);
        assert_eq!(files[0].first_seq, 4, "the three oldest were removed");
        assert_eq!(
            journal.append(&record(when(8)), when(8)).expect("append"),
            9
        );
    }

    #[test]
    fn a_full_file_rolls_over() {
        let dir = Dir::new("full");
        let journal_dir = dir.0.join(JOURNAL_DIR);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&journal_dir)
            .expect("dir");
        let now = when(0);
        let name = format!("{}-{:020}.jsonl", clock::day(now).iso(), 1);
        let path = journal_dir.join(&name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .expect("seed");
        file.set_len(FILE_LIMIT - 10).expect("grow");
        drop(file);

        let mut journal = Journal::new(dir.0.clone(), euid());
        journal.append(&record(now), now).expect("append");
        assert_eq!(list(&journal_dir).expect("list").len(), 2);
        // At most the newline that closes its torn (all-zero) tail; the new
        // line went to the new file.
        assert!(
            std::fs::metadata(&path).expect("stat").len() <= FILE_LIMIT - 9,
            "the full file was not appended to"
        );
    }

    #[test]
    fn an_unprotected_state_directory_makes_the_journal_unavailable() {
        let dir = Dir::new("open-dir");
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let mut journal = Journal::new(dir.0.clone(), euid());
        assert!(matches!(
            journal.append(&record(when(0)), when(0)),
            Err(JournalError::Unprotected(..))
        ));
        assert!(!journal.writable());
        assert!(
            !dir.0.join(JOURNAL_DIR).exists(),
            "nothing is created in an unprotected directory"
        );

        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        journal
            .append(&record(when(0)), when(0))
            .expect("recovers once repaired");
        assert!(journal.writable());
    }

    #[test]
    fn protection_is_rechecked_while_the_file_is_open() {
        let dir = Dir::new("recheck");
        let mut journal = Journal::new(dir.0.clone(), euid());
        let now = when(0);
        journal.append(&record(now), now).expect("append");
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(journal.append(&record(now), now).is_err());
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o700)).expect("chmod");
        journal.append(&record(now), now).expect("recovers");
    }

    #[test]
    fn an_unlinked_file_is_not_written_to() {
        let dir = Dir::new("unlinked");
        let mut journal = Journal::new(dir.0.clone(), euid());
        let now = when(0);
        journal.append(&record(now), now).expect("append");
        let journal_dir = dir.0.join(JOURNAL_DIR);
        for entry in list(&journal_dir).expect("list") {
            std::fs::remove_file(journal_dir.join(entry.name)).expect("unlink");
        }
        let seq = journal.append(&record(now), now).expect("append");
        let text: String = list(&journal_dir)
            .expect("list")
            .iter()
            .map(|entry| std::fs::read_to_string(journal_dir.join(&entry.name)).expect("read"))
            .collect();
        assert!(
            text.contains(&format!("\"seq\":{seq},")),
            "the line landed on disk"
        );
    }

    #[test]
    fn a_symlinked_journal_directory_is_refused() {
        let dir = Dir::new("symlink");
        let elsewhere = Dir::new("symlink-target");
        std::os::unix::fs::symlink(&elsewhere.0, dir.0.join(JOURNAL_DIR)).expect("link");
        let mut journal = Journal::new(dir.0.clone(), euid());
        assert!(matches!(
            journal.append(&record(when(0)), when(0)),
            Err(JournalError::Unprotected(..))
        ));
        assert_eq!(
            std::fs::read_dir(&elsewhere.0).expect("read").count(),
            0,
            "nothing is written through the link"
        );
    }

    #[test]
    fn a_symlinked_journal_file_is_refused() {
        let dir = Dir::new("filelink");
        let journal_dir = dir.0.join(JOURNAL_DIR);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&journal_dir)
            .expect("dir");
        let target = dir.0.join("target");
        std::fs::write(&target, b"").expect("target");
        let now = when(0);
        let name = format!("{}-{:020}.jsonl", clock::day(now).iso(), 1);
        std::os::unix::fs::symlink(&target, journal_dir.join(name)).expect("link");
        let mut journal = Journal::new(dir.0.clone(), euid());
        assert!(journal.append(&record(now), now).is_err());
        assert_eq!(std::fs::read(&target).expect("read"), b"");
    }

    #[test]
    fn unrelated_files_are_left_alone() {
        let dir = Dir::new("unrelated");
        let journal_dir = dir.0.join(JOURNAL_DIR);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&journal_dir)
            .expect("dir");
        std::fs::write(journal_dir.join("notes.txt"), b"keep").expect("seed");
        let mut journal = Journal::new(dir.0.clone(), euid());
        for day in 0..8 {
            journal
                .append(&record(when(day)), when(day))
                .expect("append");
        }
        assert_eq!(
            std::fs::read(journal_dir.join("notes.txt")).expect("kept"),
            b"keep"
        );
    }

    #[test]
    fn names_parse_strictly() {
        assert!(parse_name("2026-09-24-00000000000000000001.jsonl").is_some());
        for bad in [
            "2026-09-24-1.jsonl",
            "2026-09-24-00000000000000000001.json",
            "2026-09-24-0000000000000000000x.jsonl",
            "../2026-09-24-00000000000000000001.jsonl",
            "2026_09_24-00000000000000000001.jsonl",
        ] {
            assert!(parse_name(bad).is_none(), "{bad}");
        }
    }
}
