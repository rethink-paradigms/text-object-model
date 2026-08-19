// ── Standalone Daemon Binary ─────────────────────────────────────────────────
//
// Runs the daemon in its own OS process. The main `text-runtime` CLI opens a
// client Runtime before dispatching subcommands, which is undesirable for
// daemon-only deployments and for tests that need signal isolation.
//
// This binary goes straight to the daemon: no client Runtime is opened, and
// SIGHUP hot-reload only ever affects this single process.
//
// The daemon e2e tests use it so the SIGHUP reload scenario can be isolated
// from the other in-process daemons running in the same test binary — signals
// are process-global, so a SIGHUP sent to `Pid::this()` inside the test would
// hit every daemon and corrupt unrelated workspaces.

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// text-runtime daemon (standalone process).
#[derive(Parser)]
#[command(name = "text-runtime-daemon")]
struct Cli {
    /// Path to config file (default: $XDG_CONFIG_HOME/text-runtime/config.toml).
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Log to stderr at info by default (override with RUST_LOG).
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .try_init()
        .ok();

    let cli = Cli::parse();
    let config = text_runtime::daemon::config::load_config(cli.config.as_deref())?;
    text_runtime::daemon::run(config).await?;
    Ok(())
}
