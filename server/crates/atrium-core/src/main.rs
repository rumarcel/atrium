//! Entry point for Atrium Core.
//!
//! Thin on purpose: everything testable lives in the library target so it can
//! be exercised without spawning a process.
//!
//! ```text
//! atrium-core                 run the service
//! atrium-core init-identity   create the server identity (installer only)
//! atrium-core --version
//! ```

use std::process::ExitCode;

use atrium_core::certificate::AddressSet;
use atrium_core::init::{self, InitOutcome};
use atrium_core::layout::Layout;

/// `init-identity` found a complete identity and changed nothing.
const EXIT_ALREADY_INITIALIZED: u8 = 3;
/// Unknown arguments.
const EXIT_USAGE: u8 = 2;

const USAGE: &str = "usage: atrium-core [init-identity | --version | --help]";

fn main() -> ExitCode {
    // Everything Core creates is private unless it says otherwise, whatever
    // umask it inherited. The unit sets UMask=0077 too; this covers the
    // installer's `init-identity` invocation and a hand-started binary.
    nix::sys::stat::umask(nix::sys::stat::Mode::from_bits_truncate(0o077));

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        [] => serve(),
        ["init-identity"] => init_identity(),
        ["--version"] => {
            println!("atrium-core {}", atrium_core::VERSION);
            ExitCode::SUCCESS
        }
        ["--help"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(EXIT_USAGE)
        }
    }
}

fn serve() -> ExitCode {
    let layout = Layout::from_env();
    // Configuration chooses the log level, so it is read before logging
    // starts — but never as root, which `run` refuses anyway.
    let config = match (&layout, nix::unistd::geteuid().is_root()) {
        (Ok(layout), false) => Some(atrium_core::config::load(layout)),
        _ => None,
    };
    let level = config
        .as_ref()
        .and_then(|config| config.as_ref().ok())
        .map(|config| config.log_level);
    atrium_core::init_logging(level);

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(
                event = "startup_failed",
                component = "atrium-core",
                reason = %error,
                "the async runtime could not be created"
            );
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(atrium_core::run(layout, config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(
                event = "startup_failed",
                component = "atrium-core",
                reason = %error,
                "atrium-core refused to start"
            );
            ExitCode::FAILURE
        }
    }
}

/// Plain text, not JSON: the reader is an installer script or a person.
fn init_identity() -> ExitCode {
    if let Err(refusal) = atrium_core::guard::deny_root(nix::unistd::geteuid().as_raw()) {
        eprintln!("atrium-core init-identity: {refusal}");
        eprintln!(
            "run it as the atrium user, e.g. setpriv --reuid=atrium --regid=atrium --clear-groups"
        );
        return ExitCode::FAILURE;
    }
    let layout = match Layout::from_env() {
        Ok(layout) => layout,
        Err(error) => {
            eprintln!("atrium-core init-identity: {error}");
            return ExitCode::FAILURE;
        }
    };
    let addresses = AddressSet::current().unwrap_or_else(|error| {
        eprintln!(
            "atrium-core init-identity: could not list addresses ({error}); the first \
             certificate names only localhost and the loopbacks, and Core reissues it once \
             addresses are readable"
        );
        AddressSet::default()
    });

    match init::run(&layout, &addresses, time::OffsetDateTime::now_utc()) {
        Ok(InitOutcome::Created {
            server_id,
            pin,
            certificate,
        }) => {
            println!("created server identity");
            println!("server_id={server_id}");
            println!("spki_sha256={pin}");
            println!("certificate_serial={}", certificate.serial);
            ExitCode::SUCCESS
        }
        Ok(InitOutcome::AlreadyInitialized { server_id, pin }) => {
            eprintln!("atrium-core init-identity: already initialized; nothing was changed");
            println!("server_id={server_id}");
            println!("spki_sha256={pin}");
            ExitCode::from(EXIT_ALREADY_INITIALIZED)
        }
        Err(refusal) => {
            eprintln!("atrium-core init-identity: {refusal}");
            ExitCode::FAILURE
        }
    }
}
