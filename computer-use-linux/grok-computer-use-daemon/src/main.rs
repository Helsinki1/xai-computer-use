//! The Grok Computer Use daemon for Linux/X11, mirroring `AppMain.swift`:
//! singleton lease first, then receipt recovery, then the socket server.

mod peer_verifier;
mod receipt_store;
mod singleton;
mod socket_server;
mod x11_driver;

use std::sync::{Arc, Mutex};

use computer_use_core::paths;
use computer_use_core::runtime::{ComputerUseRuntime, SystemClock, UuidIdentifierGenerator};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("grok-computer-use-daemon: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> computer_use_core::models::Result<()> {
    // The singleton lease must be held before permissions, receipt recovery,
    // or any filesystem mutation.
    let runtime_directory = paths::runtime_directory()?;
    let singleton_lease = Arc::new(singleton::acquire(&runtime_directory)?);

    let receipts = receipt_store::DurableReceiptStore::open(&paths::receipts_directory()?)?;
    let driver = x11_driver::X11Driver::connect()?;
    let runtime = ComputerUseRuntime::new(
        Box::new(driver),
        Box::new(receipts),
        Box::new(SystemClock),
        Box::new(UuidIdentifierGenerator),
        30.0,
    )?;

    let peer_verifier = Arc::new(peer_verifier::ProcPeerVerifier::with_default_allowlist()?);
    let socket_path = socket_server::prepared_socket_path()?;
    let server = socket_server::AgentSocketServer::bind(
        &socket_path,
        Arc::new(Mutex::new(runtime)),
        peer_verifier,
        singleton_lease,
    )?;
    eprintln!(
        "grok-computer-use-daemon: serving {}",
        socket_path.display()
    );
    server.run();
    Ok(())
}
