//! The relay-side daemon client, mirroring `AgentClient.swift`: one
//! authenticated connection per relay process, strictly monotonic sequences,
//! and permanent fail-closed poisoning on any transport or authentication
//! fault — trusted calls never reconnect or replay.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

use base64::Engine as _;
use uuid::Uuid;

use computer_use_core::catalog;
use computer_use_core::mcp::ToolCaller;
use computer_use_core::models::{
    ComputerUseError, JsonObject, Result, ToolCallContext, ToolExecutionResult,
};
use computer_use_core::protocol::{self, AgentRequest, AgentRequestKind, AgentResponse};

const SOCKET_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub struct AgentClient {
    stream: UnixStream,
    client_identifier: String,
    session_identifier: String,
    session_key: Vec<u8>,
    next_sequence: u64,
    poisoned: bool,
}

fn transport_failure() -> ComputerUseError {
    ComputerUseError::StateUnavailable(
        "The computer-use daemon connection failed and will not be reused.".to_owned(),
    )
}

impl AgentClient {
    pub fn connect(client_identifier: String) -> Result<Self> {
        let socket_path = computer_use_core::paths::socket_path()?;
        let stream = UnixStream::connect(&socket_path).map_err(|_| {
            ComputerUseError::StateUnavailable(
                "The Grok Computer Use daemon is not running.".to_owned(),
            )
        })?;
        stream
            .set_read_timeout(Some(SOCKET_TIMEOUT))
            .and_then(|_| stream.set_write_timeout(Some(SOCKET_TIMEOUT)))
            .map_err(|_| transport_failure())?;

        let mut key = vec![0u8; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut key);
        let request_identifier = Uuid::new_v4().to_string();
        let mut request = AgentRequest::new(
            request_identifier.clone(),
            client_identifier.clone(),
            AgentRequestKind::Initialize,
        );
        request.session_secret = Some(base64::engine::general_purpose::STANDARD.encode(&key));
        request.validate()?;

        let mut client = Self {
            stream,
            client_identifier,
            session_identifier: String::new(),
            session_key: key,
            next_sequence: 1,
            poisoned: false,
        };
        client.write_frame(&request)?;
        let response = client.read_response()?;
        let session_identifier = response
            .session_identifier
            .clone()
            .ok_or_else(transport_failure)?;
        let proof = response
            .authentication_tag
            .as_deref()
            .ok_or_else(transport_failure)?;
        let proven = response.request_identifier == request_identifier
            && response.pong == Some(true)
            && protocol::verify_initialization_proof(
                proof,
                &client.session_key,
                &client.client_identifier,
                &request_identifier,
                &session_identifier,
            );
        if !proven {
            return Err(transport_failure());
        }
        client.session_identifier = session_identifier;
        Ok(client)
    }

    fn exchange(&mut self, request: AgentRequest) -> Result<AgentResponse> {
        if self.poisoned {
            return Err(transport_failure());
        }
        let outcome = self.exchange_inner(request);
        if outcome.is_err() {
            // Fail closed permanently: an uncertain exchange must never be
            // replayed on a fresh connection.
            self.poisoned = true;
        }
        outcome
    }

    fn exchange_inner(&mut self, request: AgentRequest) -> Result<AgentResponse> {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        let request =
            request.authenticated(self.session_identifier.clone(), sequence, &self.session_key)?;
        request.validate()?;
        self.write_frame(&request)?;
        let response = self.read_response()?;
        let tag = response
            .authentication_tag
            .as_deref()
            .ok_or_else(transport_failure)?;
        let valid = response.request_identifier == request.request_identifier
            && response.session_identifier.as_deref() == Some(self.session_identifier.as_str())
            && response.sequence == Some(sequence)
            && protocol::verify_session_tag(
                tag,
                &self.session_key,
                "response-v2",
                &response.authentication_payload()?,
            );
        if !valid {
            return Err(transport_failure());
        }
        Ok(response)
    }

    fn write_frame(&mut self, request: &AgentRequest) -> Result<()> {
        let frame = protocol::encode_frame(request)?;
        self.stream
            .write_all(&frame)
            .map_err(|_| transport_failure())
    }

    fn read_response(&mut self) -> Result<AgentResponse> {
        let mut header = [0u8; 8];
        self.stream
            .read_exact(&mut header)
            .map_err(|_| transport_failure())?;
        let length = protocol::decode_length(&header)?;
        let mut payload = vec![0u8; length];
        self.stream
            .read_exact(&mut payload)
            .map_err(|_| transport_failure())?;
        protocol::decode_response(&payload)
    }

    fn tool_request(
        &self,
        name: &str,
        arguments: JsonObject,
        action_identifier: Option<String>,
    ) -> AgentRequest {
        let mut request = AgentRequest::new(
            Uuid::new_v4().to_string(),
            self.client_identifier.clone(),
            match name {
                "attest_snapshot_delivery" => AgentRequestKind::AttestSnapshotDelivery,
                "invalidate_session" => AgentRequestKind::InvalidateSession,
                "lease_heartbeat" => AgentRequestKind::LeaseHeartbeat,
                "release_operation" => AgentRequestKind::ReleaseOperation,
                _ => AgentRequestKind::CallTool,
            },
        );
        if request.kind == AgentRequestKind::CallTool {
            request.tool_name = Some(name.to_owned());
            request.action_identifier = action_identifier;
        }
        request.arguments = Some(arguments);
        request
    }
}

impl ToolCaller for AgentClient {
    fn call_tool(
        &mut self,
        name: &str,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> ToolExecutionResult {
        if !catalog::is_accepted_tool(name) {
            return ToolExecutionResult::error(&ComputerUseError::InvalidArguments(format!(
                "Unknown tool: {name}"
            )));
        }
        let request = self.tool_request(name, arguments, context.action_identifier.clone());
        match self.exchange(request) {
            Ok(response) => {
                if let Some(result) = response.tool_result {
                    result
                } else if let Some(error) = response.error {
                    ToolExecutionResult {
                        text: format!("{}: {}", error.code, error.message),
                        structured_content: None,
                        image_png: None,
                        is_error: true,
                        protected_carrier: None,
                    }
                } else {
                    ToolExecutionResult::error(&transport_failure())
                }
            }
            Err(error) => ToolExecutionResult::error(&error),
        }
    }

    fn client_disconnected(&mut self) {
        let request = self.tool_request("invalidate_session", JsonObject::new(), None);
        let _ = self.exchange(request);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.poisoned = true;
    }
}
