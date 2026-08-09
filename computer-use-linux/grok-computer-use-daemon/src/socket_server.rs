//! Framed, authenticated Unix-socket server, mirroring
//! `AgentSocketServer.swift`: per-connection HMAC session handshake, strict
//! monotonic sequence numbers, a 64-connection cap, and disconnect cleanup.

use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use uuid::Uuid;

use computer_use_core::models::{ComputerUseError, Result, ToolCallContext};
use computer_use_core::protocol::{
    self, AgentProtocolError, AgentRequest, AgentRequestKind, AgentResponse,
};
use computer_use_core::runtime::ComputerUseRuntime;

use crate::peer_verifier::PeerVerifier;
use crate::singleton::SingletonLease;

const MAX_CONNECTIONS: i32 = 64;
const SOCKET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub struct AgentSocketServer {
    listener: UnixListener,
    runtime: Arc<Mutex<ComputerUseRuntime>>,
    peer_verifier: Arc<dyn PeerVerifier>,
    // Retaining the singleton lease in every accepted connection prevents a
    // replacement daemon from starting while durable work is in flight.
    singleton_lease: Arc<SingletonLease>,
    connection_slots: Arc<AtomicI32>,
}

impl AgentSocketServer {
    pub fn bind(
        socket_path: &Path,
        runtime: Arc<Mutex<ComputerUseRuntime>>,
        peer_verifier: Arc<dyn PeerVerifier>,
        singleton_lease: Arc<SingletonLease>,
    ) -> Result<Self> {
        let directory = socket_path.parent().ok_or_else(|| {
            ComputerUseError::StateUnavailable("The app-agent socket path is invalid.".to_owned())
        })?;
        crate::receipt_store::prepare_private_directory(directory)?;
        remove_stale_socket(socket_path)?;
        let listener = UnixListener::bind(socket_path).map_err(|_| {
            ComputerUseError::StateUnavailable(
                "The app-agent socket could not be bound.".to_owned(),
            )
        })?;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600)).map_err(
            |_| {
                ComputerUseError::PermissionDenied(
                    "The app-agent socket could not be secured.".to_owned(),
                )
            },
        )?;
        Ok(Self {
            listener,
            runtime,
            peer_verifier,
            singleton_lease,
            connection_slots: Arc::new(AtomicI32::new(MAX_CONNECTIONS)),
        })
    }

    /// Runs the accept loop on the calling thread until the listener fails.
    pub fn run(&self) {
        loop {
            let Ok((stream, _)) = self.listener.accept() else {
                return;
            };
            if self
                .connection_slots
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |slots| {
                    (slots > 0).then_some(slots - 1)
                })
                .is_err()
            {
                drop(stream);
                continue;
            }
            let slots = Arc::clone(&self.connection_slots);
            if self.peer_verifier.verify(&stream).is_err()
                || stream.set_read_timeout(Some(SOCKET_TIMEOUT)).is_err()
                || stream.set_write_timeout(Some(SOCKET_TIMEOUT)).is_err()
            {
                slots.fetch_add(1, Ordering::SeqCst);
                drop(stream);
                continue;
            }
            let runtime = Arc::clone(&self.runtime);
            let lease = Arc::clone(&self.singleton_lease);
            std::thread::spawn(move || {
                let mut connection = AgentConnection::new(stream, runtime, lease);
                connection.run();
                slots.fetch_add(1, Ordering::SeqCst);
            });
        }
    }
}

fn remove_stale_socket(path: &Path) -> Result<()> {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    use std::os::unix::fs::FileTypeExt;
    use std::os::unix::fs::MetadataExt;
    if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
        return Err(ComputerUseError::PermissionDenied(
            "Refusing to replace a non-socket or foreign app-agent path.".to_owned(),
        ));
    }
    // A live listener must never be replaced.
    match UnixStream::connect(path) {
        Ok(_) => {
            return Err(ComputerUseError::StateUnavailable(
                "Another app-agent listener is already active.".to_owned(),
            ));
        }
        Err(error)
            if error.kind() == std::io::ErrorKind::ConnectionRefused
                || error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(ComputerUseError::StateUnavailable(
                "The existing app-agent socket could not be proven stale.".to_owned(),
            ));
        }
    }
    let Ok(current) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    if current.ino() != metadata.ino()
        || current.dev() != metadata.dev()
        || !current.file_type().is_socket()
        || current.uid() != unsafe { libc::geteuid() }
    {
        return Err(ComputerUseError::PermissionDenied(
            "The app-agent socket changed during stale-socket validation.".to_owned(),
        ));
    }
    std::fs::remove_file(path).map_err(|_| {
        ComputerUseError::StateUnavailable(
            "The stale app-agent socket could not be removed.".to_owned(),
        )
    })
}

struct AgentConnection {
    stream: UnixStream,
    runtime: Arc<Mutex<ComputerUseRuntime>>,
    _singleton_lease: Arc<SingletonLease>,
    bound_client_identifier: Option<String>,
    runtime_client_identifier: Option<String>,
    session_identifier: Option<String>,
    session_key: Option<Vec<u8>>,
    last_sequence: u64,
}

impl AgentConnection {
    fn new(
        stream: UnixStream,
        runtime: Arc<Mutex<ComputerUseRuntime>>,
        singleton_lease: Arc<SingletonLease>,
    ) -> Self {
        Self {
            stream,
            runtime,
            _singleton_lease: singleton_lease,
            bound_client_identifier: None,
            runtime_client_identifier: None,
            session_identifier: None,
            session_key: None,
            last_sequence: 0,
        }
    }

    fn run(&mut self) {
        let outcome = self.serve();
        if let Some(client) = self.runtime_client_identifier.take() {
            if let Ok(mut runtime) = self.runtime.lock() {
                runtime.client_disconnected(&client);
            }
        }
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        drop(outcome);
    }

    fn serve(&mut self) -> Result<()> {
        loop {
            let Some(header) = self.read_exactly(8)? else {
                return Ok(());
            };
            let length = protocol::decode_length(&header)?;
            let Some(payload) = self.read_exactly(length)? else {
                return Ok(());
            };
            let request = protocol::decode_request(&payload)?;
            if request.kind == AgentRequestKind::Initialize {
                self.initialize(&request)?;
                continue;
            }
            let (session_identifier, key, sequence) = self.authenticate(&request)?;
            self.last_sequence = sequence;
            let response = self.handle(&request);
            let authenticated = response.authenticated(session_identifier, sequence, &key)?;
            self.write_all(&protocol::encode_frame(&authenticated)?)?;
        }
    }

    fn initialize(&mut self, request: &AgentRequest) -> Result<()> {
        let key = request
            .session_secret
            .as_deref()
            .and_then(|secret| {
                base64::engine::general_purpose::STANDARD
                    .decode(secret)
                    .ok()
            })
            .filter(|key| key.len() == 32);
        let (Some(key), None, None) = (key, &self.session_key, &self.bound_client_identifier)
        else {
            return Err(ComputerUseError::PermissionDenied(
                "Invalid app-agent session initialization.".to_owned(),
            ));
        };
        let session_identifier = Uuid::new_v4().to_string();
        let proof = protocol::initialization_proof(
            &key,
            &request.client_identifier,
            &request.request_identifier,
            &session_identifier,
        )?;
        self.bound_client_identifier = Some(request.client_identifier.clone());
        self.runtime_client_identifier = Some(format!(
            "{}:{}",
            request.client_identifier, session_identifier
        ));
        self.session_identifier = Some(session_identifier.clone());
        self.session_key = Some(key);
        let mut response = AgentResponse::new(request.request_identifier.clone());
        response.pong = Some(true);
        response.session_identifier = Some(session_identifier);
        response.authentication_tag = Some(proof);
        self.write_all(&protocol::encode_frame(&response)?)
    }

    fn authenticate(&self, request: &AgentRequest) -> Result<(String, Vec<u8>, u64)> {
        let denied = || {
            ComputerUseError::PermissionDenied(
                "Invalid authenticated app-agent request.".to_owned(),
            )
        };
        let bound = self.bound_client_identifier.as_deref().ok_or_else(denied)?;
        let session = self.session_identifier.as_deref().ok_or_else(denied)?;
        let key = self.session_key.as_deref().ok_or_else(denied)?;
        let sequence = request.sequence.ok_or_else(denied)?;
        let tag = request.authentication_tag.as_deref().ok_or_else(denied)?;
        let valid = bound == request.client_identifier
            && request.session_identifier.as_deref() == Some(session)
            && sequence == self.last_sequence + 1
            && protocol::verify_session_tag(
                tag,
                key,
                "request-v2",
                &request.authentication_payload()?,
            );
        if !valid {
            return Err(denied());
        }
        Ok((session.to_owned(), key.to_vec(), sequence))
    }

    fn handle(&self, request: &AgentRequest) -> AgentResponse {
        let mut response = AgentResponse::new(request.request_identifier.clone());
        match request.kind {
            AgentRequestKind::Initialize => {
                response.error = Some(AgentProtocolError {
                    code: "invalid_request".to_owned(),
                    message: "Session is already initialized.".to_owned(),
                });
            }
            AgentRequestKind::Ping => {
                response.pong = Some(true);
            }
            AgentRequestKind::CallTool => {
                let (Some(tool_name), Some(arguments)) = (&request.tool_name, &request.arguments)
                else {
                    response.error = Some(AgentProtocolError {
                        code: "invalid_request".to_owned(),
                        message: "Invalid tool request.".to_owned(),
                    });
                    return response;
                };
                let context = ToolCallContext {
                    client_identifier: self.runtime_client(request),
                    action_identifier: request.action_identifier.clone(),
                };
                let result = self.runtime.lock().expect("runtime lock").call_tool(
                    tool_name,
                    arguments.clone(),
                    &context,
                );
                response.tool_result = Some(result);
            }
            AgentRequestKind::ActionOutcome => {
                let Some(receipt_identifier) = &request.receipt_identifier else {
                    response.error = Some(AgentProtocolError {
                        code: "invalid_request".to_owned(),
                        message: "Invalid outcome request.".to_owned(),
                    });
                    return response;
                };
                response.receipt = self
                    .runtime
                    .lock()
                    .expect("runtime lock")
                    .action_outcome(receipt_identifier);
            }
            AgentRequestKind::AttestSnapshotDelivery => {
                response.tool_result = Some(self.lifecycle(request, "attest_snapshot_delivery"));
            }
            AgentRequestKind::InvalidateSession => {
                response.tool_result = Some(self.lifecycle(request, "invalidate_session"));
            }
            AgentRequestKind::LeaseHeartbeat => {
                response.tool_result = Some(self.lifecycle(request, "lease_heartbeat"));
            }
            AgentRequestKind::ReleaseOperation => {
                response.tool_result = Some(self.lifecycle(request, "release_operation"));
            }
        }
        response
    }

    fn lifecycle(
        &self,
        request: &AgentRequest,
        tool_name: &str,
    ) -> computer_use_core::models::ToolExecutionResult {
        let context = ToolCallContext {
            client_identifier: self.runtime_client(request),
            action_identifier: None,
        };
        self.runtime.lock().expect("runtime lock").call_tool(
            tool_name,
            request.arguments.clone().unwrap_or_default(),
            &context,
        )
    }

    fn runtime_client(&self, request: &AgentRequest) -> String {
        self.runtime_client_identifier
            .clone()
            .unwrap_or_else(|| request.client_identifier.clone())
    }

    fn read_exactly(&mut self, count: usize) -> Result<Option<Vec<u8>>> {
        let mut buffer = vec![0u8; count];
        match self.stream.read_exact(&mut buffer) {
            Ok(()) => Ok(Some(buffer)),
            Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
            Err(_) => Err(ComputerUseError::StateUnavailable(
                "The app-agent connection failed.".to_owned(),
            )),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.stream.write_all(data).map_err(|_| {
            ComputerUseError::StateUnavailable("The app-agent connection failed.".to_owned())
        })
    }
}

/// Returns the socket path the daemon serves, creating its directory.
pub fn prepared_socket_path() -> Result<PathBuf> {
    let path = computer_use_core::paths::socket_path()?;
    if let Some(parent) = path.parent() {
        crate::receipt_store::prepare_private_directory(parent)?;
    }
    Ok(path)
}
