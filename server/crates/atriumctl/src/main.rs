//! `atriumctl` — the console tool.
//!
//! It runs on the server, as root, and owns the operations that must not be
//! reachable over the network: issuing a pairing secret, rotating the identity
//! key, restoring a corrupt database. Keeping them here rather than in Core is
//! the same invariant the filesystem enforces — the long-running network
//! service contains no code that can replace the identity or overwrite the
//! state.
//!
//! In M1A it has no commands, because none of those subsystems exist yet. It
//! reports its version and says so plainly rather than offering a command that
//! would do nothing.

use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
atriumctl — Atrium console tool

Usage:
  atriumctl --version
  atriumctl --help

No operational commands exist yet. `pair`, `rotate-identity`, `restore` and
`diagnostics` arrive with the subsystems they operate on; see
docs/M1-IMPLEMENTATION-PLAN.md section 17.
";

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();

    match arguments.as_slice() {
        [] => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        [flag] if flag == "--version" || flag == "-V" => {
            println!("atriumctl {VERSION}");
            ExitCode::SUCCESS
        }
        [flag] if flag == "--help" || flag == "-h" => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("atriumctl: unrecognised arguments");
            eprint!("{USAGE}");
            // 2 is the conventional usage error, distinct from a runtime
            // failure, so a script can tell the two apart.
            ExitCode::from(2)
        }
    }
}
