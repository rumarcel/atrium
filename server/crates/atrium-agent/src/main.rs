//! Entry point for Atrium Agent.

fn main() -> std::process::ExitCode {
    atrium_agent::init_logging();

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            tracing::error!(
                event = "startup_failed",
                component = "atrium-agent",
                reason = %error,
                "the async runtime could not be created"
            );
            return std::process::ExitCode::FAILURE;
        }
    };

    match runtime.block_on(atrium_agent::run()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(
                event = "startup_failed",
                component = "atrium-agent",
                reason = %error,
                "atrium-agent refused to start"
            );
            std::process::ExitCode::FAILURE
        }
    }
}
