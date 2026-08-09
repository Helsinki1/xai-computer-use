//! Linux peer verification for accepted agent connections.
//!
//! This is the documented weaker stand-in for the macOS audit-token +
//! code-signature checks: the daemon requires the connecting peer to be the
//! same user and requires `/proc/<pid>/exe` to resolve to an allowlisted
//! executable path (the fixed relay install path by default). `/proc` exe
//! resolution is subject to an inherent TOCTOU window; the deployment model
//! accepts same-user trust on Linux.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;

use computer_use_core::models::{ComputerUseError, Result};
use computer_use_core::paths;

pub trait PeerVerifier: Send + Sync {
    fn verify(&self, stream: &UnixStream) -> Result<PeerIdentity>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerIdentity {
    pub process_identifier: i32,
    pub user_identifier: u32,
    pub executable: PathBuf,
}

pub struct ProcPeerVerifier {
    allowed_executables: Vec<PathBuf>,
}

impl ProcPeerVerifier {
    /// The production allowlist: the installed relay binary only. The
    /// `GROK_COMPUTER_USE_EXTRA_PEER_EXECUTABLE` environment variable extends
    /// it for local development and CI fixtures.
    pub fn with_default_allowlist() -> Result<Self> {
        let mut allowed = vec![paths::installed_relay_path()?];
        if let Some(extra) = std::env::var_os("GROK_COMPUTER_USE_EXTRA_PEER_EXECUTABLE") {
            if !extra.is_empty() {
                allowed.push(PathBuf::from(extra));
            }
        }
        Ok(Self {
            allowed_executables: allowed,
        })
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(allowed_executables: Vec<PathBuf>) -> Self {
        Self {
            allowed_executables,
        }
    }
}

fn denied(message: &str) -> ComputerUseError {
    ComputerUseError::PermissionDenied(message.to_owned())
}

pub fn peer_credentials(stream: &UnixStream) -> Result<(i32, u32)> {
    use std::os::fd::AsRawFd;
    let mut credentials: libc::ucred = unsafe { std::mem::zeroed() };
    let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let outcome = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if outcome != 0 || length as usize != std::mem::size_of::<libc::ucred>() {
        return Err(denied("The peer credentials could not be read."));
    }
    Ok((credentials.pid, credentials.uid))
}

impl PeerVerifier for ProcPeerVerifier {
    fn verify(&self, stream: &UnixStream) -> Result<PeerIdentity> {
        let (pid, uid) = peer_credentials(stream)?;
        if uid != unsafe { libc::geteuid() } {
            return Err(denied("The peer is not the same user."));
        }
        if pid <= 0 {
            return Err(denied("The peer process is unavailable."));
        }
        let executable = std::fs::read_link(format!("/proc/{pid}/exe"))
            .map_err(|_| denied("The peer executable could not be resolved."))?;
        if !self
            .allowed_executables
            .iter()
            .any(|allowed| allowed == &executable)
        {
            return Err(denied("The peer executable is not the trusted relay."));
        }
        Ok(PeerIdentity {
            process_identifier: pid,
            user_identifier: uid,
            executable,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_user_allowlisted_peer_is_accepted_and_foreign_exe_is_rejected() {
        let (left, _right) = UnixStream::pair().unwrap();
        let own_executable = std::fs::read_link("/proc/self/exe").unwrap();

        let accepting = ProcPeerVerifier::new(vec![own_executable.clone()]);
        let identity = accepting.verify(&left).unwrap();
        assert_eq!(identity.process_identifier, std::process::id() as i32);
        assert_eq!(identity.executable, own_executable);

        let rejecting = ProcPeerVerifier::new(vec![PathBuf::from("/usr/bin/definitely-not-us")]);
        assert!(rejecting.verify(&left).is_err());
    }
}
