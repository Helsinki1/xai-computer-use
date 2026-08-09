//! Direct-parent verification, mirroring `ParentProcessVerifier.swift` with
//! the documented weaker Linux trust model: the relay's direct parent must be
//! a same-user process whose executable is an allowlisted Grok host.

use std::path::PathBuf;

use computer_use_core::models::{ComputerUseError, Result};

const DEFAULT_PARENT_BASENAMES: [&str; 2] = ["grok", "grok-bin"];

fn denied() -> ComputerUseError {
    ComputerUseError::PermissionDenied(
        "The relay was not launched by a trusted Grok host.".to_owned(),
    )
}

pub fn verify() -> Result<()> {
    let parent = unsafe { libc::getppid() };
    if parent <= 1 {
        return Err(denied());
    }
    let proc_dir = PathBuf::from(format!("/proc/{parent}"));
    let metadata = std::fs::metadata(&proc_dir).map_err(|_| denied())?;
    use std::os::unix::fs::MetadataExt;
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err(denied());
    }
    let executable = std::fs::read_link(proc_dir.join("exe")).map_err(|_| denied())?;

    if let Some(allowlist) = std::env::var_os("GROK_COMPUTER_USE_PARENT_EXECUTABLES") {
        let allowed = std::env::split_paths(&allowlist).any(|path| path == executable);
        return if allowed { Ok(()) } else { Err(denied()) };
    }
    let basename = executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(denied)?;
    if DEFAULT_PARENT_BASENAMES.contains(&basename) {
        Ok(())
    } else {
        Err(denied())
    }
}
