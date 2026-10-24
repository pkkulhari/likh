mod app;
mod audio;
mod auth;
mod config;
mod desktop;
mod transcribe;

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Hold Ctrl+Win to type with your voice on Linux/X11")]
struct Cli {
    #[arg(long, global = true)]
    config_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    runtime_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Sign in to Claude with a separate browser OAuth login.
    Login {
        #[arg(long)]
        no_browser: bool,
    },
    /// Remove Likh's locally stored credentials (does not revoke OAuth).
    Logout,
    /// Show local authentication status without revealing credentials.
    Status,
    /// Run the background voice typing app until Ctrl+C or SIGTERM.
    Run,
    /// Check desktop, microphone server, configuration, and local login.
    Doctor,
    /// Copy the last transcript retained by the running app.
    CopyLast,
}
fn main() {
    if let Err(error) = execute() {
        eprintln!("likh: {error:#}");
        std::process::exit(1);
    }
}
fn execute() -> Result<()> {
    let cli = Cli::parse();
    let dir = cli.config_dir.map(Ok).unwrap_or_else(config::config_dir)?;
    let config = config::Config::load(&dir)?;
    let store = auth::Store { dir };
    match cli.command {
        Command::Status => print_status(&store),
        Command::Logout => {
            store.logout()?;
            println!("Local Likh credentials removed. Environment tokens are unchanged.");
        }
        Command::Doctor => {
            let mut failures = 0;
            match desktop::Desktop::open(false) {
                Ok(_) => println!("X11 + XTEST: available"),
                Err(e) => {
                    println!("X11: {e}");
                    failures += 1;
                }
            }
            match audio::Audio::probe() {
                Ok(()) => println!("PulseAudio/PipeWire: connected"),
                Err(e) => {
                    println!("Audio: {e}");
                    failures += 1;
                }
            }
            println!("Configuration: {}", store.dir.display());
            print_status(&store);
            anyhow::ensure!(failures == 0, "Some desktop checks failed");
        }
        Command::CopyLast => {
            app::copy_last(&cli.runtime_dir.map(Ok).unwrap_or_else(app::runtime_dir)?)?
        }
        command => {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()?;
            match command {
                Command::Login { no_browser } => runtime.block_on(store.login(no_browser))?,
                Command::Run => app::run(
                    config,
                    store,
                    cli.runtime_dir.map(Ok).unwrap_or_else(app::runtime_dir)?,
                    &runtime,
                )?,
                _ => unreachable!(),
            }
        }
    }
    Ok(())
}
fn print_status(store: &auth::Store) {
    if std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN").is_some() {
        println!("Authentication: environment token (not automatically refreshed)");
    } else {
        match store.load() {
            Ok(c) => println!(
                "Authentication: {}",
                if c.expires_at > auth::now() + 60 {
                    "stored token; not verified with server"
                } else {
                    "stored token needs refresh"
                }
            ),
            Err(_) => println!("Authentication: not logged in; run `likh login`"),
        }
    }
}
