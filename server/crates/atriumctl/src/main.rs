//! `atriumctl` — the console tool.
//!
//! It runs on the server, as root, and owns the operations that must not be
//! reachable over the network: issuing a pairing secret, rotating the
//! identity key and restoring the state database. Keeping them here
//! rather than in Core is the same invariant the filesystem enforces — the
//! long-running network service contains no code that can replace the
//! identity or overwrite the state.
//!
//! Every command that touches the state directory drops from root to the
//! `atrium` service user for good first, so nothing it creates there is
//! root-owned — a root-owned `-wal` file would lock Core out of its own
//! database — and nothing the service user wrote is ever parsed by a process
//! that could regain root. The one step that stays root is writing the new
//! key during a rotation, because `/etc/atrium` is root's; it happens before
//! the drop and reads only root-owned files.

use std::process::ExitCode;

#[cfg(unix)]
mod cmd;
#[cfg(unix)]
mod privilege;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
atriumctl — Atrium console tool

Usage:
  atriumctl pair                    arm pairing: show a one-time pairing code for
                                    a native Atrium client (15 minutes, one device)
  atriumctl diagnostics             what Core would find: identity, certificate,
                                    state database, backups (read-only)
  atriumctl restore --list          pre-migration backups of the state database
  atriumctl restore --from <name>   replace the state database with a backup;
                                    atrium-core must be stopped first
  atriumctl rotate-identity         replace the identity key; revokes every
                                    device and arms a fresh pairing code;
                                    atrium-core must be stopped first
  atriumctl --version
  atriumctl --help

Destructive commands ask you to type the server's name to confirm.
";

/// Conventional usage error, distinct from a runtime failure, so a script can
/// tell the two apart.
const EXIT_USAGE: u8 = 2;

fn main() -> ExitCode {
    // Whatever this creates is private unless it says otherwise, whatever
    // umask the operator's shell has.
    #[cfg(unix)]
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));

    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match arguments.as_slice() {
        [] | ["--help" | "-h"] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        ["--version" | "-V"] => {
            println!("atriumctl {VERSION}");
            ExitCode::SUCCESS
        }
        #[cfg(unix)]
        ["pair"] => cmd::pair::run(),
        #[cfg(unix)]
        ["diagnostics"] => cmd::diagnostics::run(),
        #[cfg(unix)]
        ["restore", "--list"] => cmd::restore::list(),
        #[cfg(unix)]
        ["restore", "--from", name] => cmd::restore::from(name),
        #[cfg(unix)]
        ["rotate-identity"] => cmd::rotate_identity::run(),
        #[cfg(not(unix))]
        ["pair" | "diagnostics" | "restore" | "rotate-identity", ..] => {
            eprintln!("atriumctl: this command runs on the Linux server only");
            ExitCode::FAILURE
        }
        _ => {
            eprintln!("atriumctl: unrecognised arguments");
            eprint!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}
