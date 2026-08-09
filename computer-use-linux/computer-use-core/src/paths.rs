//! Fixed filesystem locations for the Linux daemon and relay, the analogue of
//! `ComputerUsePaths` in `AgentProtocol.swift`.
//!
//! Layout (all per-user, all mode-0700 directories):
//!   support:  ~/.local/share/grok-computer-use        (receipts, key file)
//!   runtime:  $XDG_RUNTIME_DIR/grok-computer-use      (socket, instance lock)
//!   install:  ~/.local/libexec/grok-computer-use      (daemon + relay binaries)

use std::path::PathBuf;

use crate::models::{ComputerUseError, Result};

pub const APPLICATION_IDENTIFIER: &str = "grok-computer-use";
pub const DAEMON_EXECUTABLE_NAME: &str = "grok-computer-use-daemon";
pub const RELAY_EXECUTABLE_NAME: &str = "grok-computer-use-mcp";
pub const SOCKET_FILE_NAME: &str = "agent-v2.sock";
/// Abstract-namespace name whose bind serves as the process-singleton
/// guarantee (the Linux analogue of the macOS named bootstrap port).
pub const SINGLETON_ABSTRACT_NAME: &str = "grok-computer-use-v2-singleton";

fn home_directory() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            ComputerUseError::StateUnavailable("The HOME directory is unavailable.".to_owned())
        })
}

pub fn support_directory() -> Result<PathBuf> {
    Ok(home_directory()?
        .join(".local/share")
        .join(APPLICATION_IDENTIFIER))
}

pub fn receipts_directory() -> Result<PathBuf> {
    Ok(support_directory()?.join("receipts"))
}

pub fn runtime_directory() -> Result<PathBuf> {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|directory| !directory.is_empty()) {
        Some(directory) => Ok(PathBuf::from(directory).join(APPLICATION_IDENTIFIER)),
        // Fall back to the support directory when no runtime dir exists
        // (e.g. bare CI shells); it has the same 0700/0600 guarantees.
        None => support_directory(),
    }
}

pub fn socket_path() -> Result<PathBuf> {
    Ok(runtime_directory()?.join(SOCKET_FILE_NAME))
}

pub fn install_directory() -> Result<PathBuf> {
    Ok(home_directory()?
        .join(".local/libexec")
        .join(APPLICATION_IDENTIFIER))
}

pub fn installed_daemon_path() -> Result<PathBuf> {
    Ok(install_directory()?.join(DAEMON_EXECUTABLE_NAME))
}

pub fn installed_relay_path() -> Result<PathBuf> {
    Ok(install_directory()?.join(RELAY_EXECUTABLE_NAME))
}
