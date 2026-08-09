//! Process-singleton lease, mirroring the macOS bootstrap-port + flock pair.
//!
//! The abstract-namespace Unix socket bind is the trust root (the kernel
//! guarantees at most one holder per network namespace, like the macOS named
//! bootstrap port); the flock on `agent-v2.lock` remains as filesystem
//! serialization. The lease must be acquired before permissions, receipt
//! recovery, or any filesystem mutation.

use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::net::UnixListener;
use std::path::Path;

use computer_use_core::models::{ComputerUseError, Result};
use computer_use_core::paths::SINGLETON_ABSTRACT_NAME;

pub struct SingletonLease {
    _abstract_socket: UnixListener,
    _lock_file: OwnedFd,
}

fn unavailable() -> ComputerUseError {
    ComputerUseError::StateUnavailable(
        "Another Grok Computer Use daemon instance is active.".to_owned(),
    )
}

pub fn acquire(runtime_directory: &Path) -> Result<SingletonLease> {
    let abstract_socket = bind_abstract(SINGLETON_ABSTRACT_NAME).map_err(|_| unavailable())?;
    crate::receipt_store::prepare_private_directory(runtime_directory)?;
    let lock_path = runtime_directory.join("agent-v2.lock");
    let lock_file = acquire_lock(&lock_path)?;
    Ok(SingletonLease {
        _abstract_socket: abstract_socket,
        _lock_file: lock_file,
    })
}

fn bind_abstract(name: &str) -> std::io::Result<UnixListener> {
    use std::os::linux::net::SocketAddrExt;
    let address = std::os::unix::net::SocketAddr::from_abstract_name(name.as_bytes())?;
    UnixListener::bind_addr(&address)
}

fn acquire_lock(path: &Path) -> Result<OwnedFd> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;
    use std::os::unix::ffi::OsStrExt;
    let c_path = CString::new(path.as_os_str().as_bytes()).map_err(|_| unavailable())?;
    let descriptor = unsafe {
        libc::open(
            c_path.as_ptr(),
            libc::O_CREAT | libc::O_RDWR | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if descriptor < 0 {
        return Err(unavailable());
    }
    let owned = unsafe { OwnedFd::from_raw_fd(descriptor) };
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    let valid = unsafe { libc::fstat(owned.as_raw_fd(), &mut status) } == 0
        && status.st_uid == unsafe { libc::geteuid() }
        && status.st_mode & libc::S_IFMT == libc::S_IFREG
        && status.st_nlink == 1
        && unsafe { libc::fchmod(owned.as_raw_fd(), 0o600) } == 0
        && unsafe { libc::flock(owned.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
    if !valid {
        return Err(unavailable());
    }
    Ok(owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_lock_holder_is_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let first = acquire_lock(&directory.path().join("agent-v2.lock")).unwrap();
        assert!(acquire_lock(&directory.path().join("agent-v2.lock")).is_err());
        drop(first);
        acquire_lock(&directory.path().join("agent-v2.lock")).unwrap();
    }
}
