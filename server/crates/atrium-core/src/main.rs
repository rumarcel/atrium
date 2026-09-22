//! Entry point for Atrium Core.
//!
//! Thin on purpose: everything testable lives in the library target so it can
//! be exercised without spawning a process.

fn main() -> std::process::ExitCode {
    atrium_core::init_logging();

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
            return std::process::ExitCode::FAILURE;
        }
    };

    match runtime.block_on(atrium_core::run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(
                event = "startup_failed",
                component = "atrium-core",
                reason = %error,
                "atrium-core refused to start"
            );
            std::process::ExitCode::FAILURE
        }
    }
}
