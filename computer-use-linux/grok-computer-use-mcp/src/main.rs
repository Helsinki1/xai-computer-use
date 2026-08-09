//! The stateless stdio MCP relay for Linux, mirroring `RelayMain.swift`:
//! verify the parent host, connect once to the daemon, serve JSON-RPC lines,
//! and invalidate the session on stdin close. No desktop state lives here.

mod agent_client;
mod parent_verifier;

use std::io::{BufRead, Write};

use computer_use_core::mcp::McpServer;
use uuid::Uuid;

fn main() -> std::process::ExitCode {
    let client_identifier = Uuid::new_v4().to_string();
    if parent_verifier::verify().is_err() {
        eprintln!("Grok Computer Use MCP relay failed to initialize.");
        return std::process::ExitCode::FAILURE;
    }
    let client = match agent_client::AgentClient::connect(client_identifier.clone()) {
        Ok(client) => client,
        Err(_) => {
            eprintln!("Grok Computer Use MCP relay failed to initialize.");
            return std::process::ExitCode::FAILURE;
        }
    };
    let mut server = McpServer::new(client, client_identifier, "grok-computer-use-linux");

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(response) = server.handle(&line) {
            let mut handle = stdout.lock();
            if handle
                .write_all(response.as_bytes())
                .and_then(|_| handle.write_all(b"\n"))
                .and_then(|_| handle.flush())
                .is_err()
            {
                break;
            }
        }
    }
    server.disconnect();
    std::process::ExitCode::SUCCESS
}
