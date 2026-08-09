//! `grok computer-use` — manage the reserved native computer-use profile.

use std::path::PathBuf;

use anyhow::{Result, bail};
use clap::Subcommand;
use serde::Serialize;

#[derive(Debug, clap::Args, Clone)]
pub struct ComputerUseArgs {
    #[command(subcommand)]
    pub command: ComputerUseCommand,
}

#[derive(Debug, Subcommand, Clone)]
pub enum ComputerUseCommand {
    /// Show policy, installation, signature, and platform readiness
    Status {
        /// Emit machine-readable JSON output
        #[arg(long)]
        json: bool,
    },
    /// Enable the reserved profile for new sessions
    Enable,
    /// Disable the reserved profile for new sessions
    Disable,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ComputerUseStatus {
    supported: bool,
    local_development: bool,
    configured: bool,
    installed: bool,
    secure_path: bool,
    signature_valid: bool,
    ready: bool,
    app_path: String,
    relay_path: String,
}

pub async fn run(args: ComputerUseArgs) -> Result<()> {
    match args.command {
        ComputerUseCommand::Status { json } => run_status(json),
        ComputerUseCommand::Enable => run_enable().await,
        ComputerUseCommand::Disable => run_disable().await,
    }
}

fn app_bundle_path() -> Result<PathBuf> {
    xai_grok_shell::util::computer_use::installation_root_path()
        .ok_or_else(|| anyhow::anyhow!("home directory is unavailable"))
}

fn collect_status() -> Result<ComputerUseStatus> {
    let app_path = app_bundle_path()?;
    let relay_path = app_path.join(xai_grok_shell::util::computer_use::RELAY_RELATIVE_PATH);
    let installed = app_path.is_dir() && relay_path.is_file();
    let secure_path = installed
        && xai_grok_shell::util::computer_use::path_chain_has_no_symlinks(&app_path, &relay_path);
    let supported = xai_grok_shell::util::computer_use::platform_supported();
    let local_development = xai_grok_shell::util::computer_use::local_development_enabled();
    let signature_valid =
        supported && secure_path && xai_grok_shell::util::computer_use::signature_valid(&app_path);
    let configured = xai_grok_shell::util::config::computer_use_enabled();
    let ready = supported && configured && installed && secure_path && signature_valid;

    Ok(ComputerUseStatus {
        supported,
        local_development,
        configured,
        installed,
        secure_path,
        signature_valid,
        ready,
        app_path: app_path.display().to_string(),
        relay_path: relay_path.display().to_string(),
    })
}

fn run_status(json: bool) -> Result<()> {
    let status = collect_status()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        let install_label = if cfg!(target_os = "linux") {
            "install root"
        } else {
            "app"
        };
        println!("Computer Use");
        println!("  supported: {}", status.supported);
        println!("  local development: {}", status.local_development);
        println!("  configured: {}", status.configured);
        println!("  installed: {}", status.installed);
        println!("  secure path: {}", status.secure_path);
        println!("  signature valid: {}", status.signature_valid);
        println!("  ready: {}", status.ready);
        println!("  {install_label}: {}", status.app_path);
        println!("  relay: {}", status.relay_path);
    }
    Ok(())
}

async fn run_enable() -> Result<()> {
    let status = collect_status()?;
    if !status.supported {
        if cfg!(target_os = "linux") {
            bail!("computer use requires a local X11 session on Linux");
        }
        bail!("computer use requires Apple Silicon and macOS 14 or newer");
    }
    if !status.installed {
        if cfg!(target_os = "linux") {
            bail!("grok-computer-use-mcp is not installed in ~/.local/libexec/grok-computer-use");
        }
        bail!("Grok Computer Use.app is not installed in ~/Applications");
    }
    if !status.secure_path {
        bail!("computer-use install root or relay path contains a symbolic link");
    }
    if !status.signature_valid {
        if cfg!(target_os = "linux") {
            bail!(
                "computer-use relay is not owned by the current user at the reserved install path"
            );
        }
        bail!("computer-use app failed code-signing or Gatekeeper verification");
    }
    xai_grok_shell::util::config::save_computer_use_enabled(true).await?;
    println!("Computer Use enabled for new sessions.");
    Ok(())
}

async fn run_disable() -> Result<()> {
    xai_grok_shell::util::config::save_computer_use_enabled(false).await?;
    println!("Computer Use disabled for new sessions.");
    Ok(())
}
