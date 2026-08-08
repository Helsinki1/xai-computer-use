//! The v2 app-agent protocol, mirroring `AgentProtocol.swift` exactly:
//! envelope validation, HMAC-SHA256 session authentication with domain
//! separation, and the length-prefixed JSON wire format.

use base64::Engine as _;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use uuid::Uuid;

use crate::catalog;
use crate::models::{ActionReceipt, ComputerUseError, JsonObject, Result, ToolExecutionResult};

pub const PROTOCOL_VERSION: u32 = 2;
/// A maximum-sized protected PNG expands to about 1.2 MB in JSON base64; keep
/// headroom without letting one header reserve tens of megabytes.
pub const MAXIMUM_FRAME_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentRequestKind {
    #[serde(rename = "initialize")]
    Initialize,
    #[serde(rename = "ping")]
    Ping,
    #[serde(rename = "call_tool")]
    CallTool,
    #[serde(rename = "action_outcome")]
    ActionOutcome,
    #[serde(rename = "attest_snapshot_delivery")]
    AttestSnapshotDelivery,
    #[serde(rename = "invalidate_session")]
    InvalidateSession,
    #[serde(rename = "lease_heartbeat")]
    LeaseHeartbeat,
    #[serde(rename = "release_operation")]
    ReleaseOperation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRequest {
    pub version: u32,
    #[serde(rename = "requestIdentifier")]
    pub request_identifier: String,
    #[serde(rename = "clientIdentifier")]
    pub client_identifier: String,
    pub kind: AgentRequestKind,
    #[serde(rename = "toolName", skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<JsonObject>,
    #[serde(rename = "actionIdentifier", skip_serializing_if = "Option::is_none")]
    pub action_identifier: Option<String>,
    #[serde(rename = "receiptIdentifier", skip_serializing_if = "Option::is_none")]
    pub receipt_identifier: Option<String>,
    #[serde(rename = "sessionIdentifier", skip_serializing_if = "Option::is_none")]
    pub session_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(rename = "authenticationTag", skip_serializing_if = "Option::is_none")]
    pub authentication_tag: Option<String>,
    #[serde(rename = "sessionSecret", skip_serializing_if = "Option::is_none")]
    pub session_secret: Option<String>,
}

fn is_valid_uuid(value: &str) -> bool {
    Uuid::parse_str(value).is_ok()
}

fn is_valid_bounded_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn decoded_base64_len(value: &str) -> Option<usize> {
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .ok()
        .map(|bytes| bytes.len())
}

impl AgentRequest {
    pub fn new(
        request_identifier: String,
        client_identifier: String,
        kind: AgentRequestKind,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_identifier,
            client_identifier,
            kind,
            tool_name: None,
            arguments: None,
            action_identifier: None,
            receipt_identifier: None,
            session_identifier: None,
            sequence: None,
            authentication_tag: None,
            session_secret: None,
        }
    }

    pub fn validate(&self) -> Result<()> {
        let invalid = |message: &str| ComputerUseError::InvalidArguments(message.to_owned());
        if self.version != PROTOCOL_VERSION
            || !is_valid_uuid(&self.request_identifier)
            || !is_valid_uuid(&self.client_identifier)
        {
            return Err(invalid("Invalid app-agent protocol envelope."));
        }

        match self.kind {
            AgentRequestKind::Initialize => {
                let secret_valid = self
                    .session_secret
                    .as_deref()
                    .and_then(decoded_base64_len)
                    .is_some_and(|len| len == 32);
                if self.tool_name.is_some()
                    || self.arguments.is_some()
                    || self.action_identifier.is_some()
                    || self.receipt_identifier.is_some()
                    || self.session_identifier.is_some()
                    || self.sequence.is_some()
                    || self.authentication_tag.is_some()
                    || !secret_valid
                {
                    return Err(invalid("Invalid app-agent initialization request."));
                }
            }
            AgentRequestKind::Ping => {
                if self.tool_name.is_some()
                    || self.arguments.is_some()
                    || self.action_identifier.is_some()
                    || self.receipt_identifier.is_some()
                {
                    return Err(invalid("Ping request contains unexpected fields."));
                }
            }
            AgentRequestKind::CallTool => {
                let tool_name = self
                    .tool_name
                    .as_deref()
                    .filter(|name| catalog::is_accepted_tool(name));
                if tool_name.is_none()
                    || self.arguments.is_none()
                    || self.receipt_identifier.is_some()
                {
                    return Err(invalid("Invalid call_tool request."));
                }
                let tool_name = tool_name.unwrap();
                if catalog::is_action_tool(tool_name) {
                    if !self
                        .action_identifier
                        .as_deref()
                        .is_some_and(is_valid_bounded_identifier)
                    {
                        return Err(invalid("Action request has no valid action identifier."));
                    }
                } else if self.action_identifier.is_some() {
                    return Err(invalid("Read-only request contains an action identifier."));
                }
            }
            AgentRequestKind::ActionOutcome => {
                if !self
                    .receipt_identifier
                    .as_deref()
                    .is_some_and(is_valid_bounded_identifier)
                    || self.tool_name.is_some()
                    || self.arguments.is_some()
                    || self.action_identifier.is_some()
                {
                    return Err(invalid("Invalid action_outcome request."));
                }
            }
            AgentRequestKind::AttestSnapshotDelivery
            | AgentRequestKind::LeaseHeartbeat
            | AgentRequestKind::ReleaseOperation => {
                if self.tool_name.is_some()
                    || self.arguments.is_none()
                    || self.action_identifier.is_some()
                    || self.receipt_identifier.is_some()
                {
                    return Err(invalid("Invalid app-agent lifecycle request."));
                }
            }
            AgentRequestKind::InvalidateSession => {
                if self.tool_name.is_some()
                    || !self.arguments.as_ref().is_some_and(JsonObject::is_empty)
                    || self.action_identifier.is_some()
                    || self.receipt_identifier.is_some()
                {
                    return Err(invalid("Invalid session invalidation request."));
                }
            }
        }

        if self.kind != AgentRequestKind::Initialize {
            let session_valid = self.session_secret.is_none()
                && self
                    .session_identifier
                    .as_deref()
                    .is_some_and(is_valid_uuid)
                && self.sequence.is_some_and(|sequence| sequence > 0)
                && self
                    .authentication_tag
                    .as_deref()
                    .and_then(decoded_base64_len)
                    .is_some_and(|len| len == 32);
            if !session_valid {
                return Err(invalid("Missing authenticated app-agent session envelope."));
            }
        }
        Ok(())
    }

    pub fn authenticated(
        mut self,
        session_identifier: String,
        sequence: u64,
        key: &[u8],
    ) -> Result<Self> {
        self.session_identifier = Some(session_identifier);
        self.sequence = Some(sequence);
        self.authentication_tag = None;
        let tag = session_tag(key, "request-v2", &self.authentication_payload()?)?;
        self.authentication_tag = Some(tag);
        Ok(self)
    }

    /// The canonical (sorted-key JSON) byte payload covered by the tag: every
    /// field except `authenticationTag` and `sessionSecret`.
    pub fn authentication_payload(&self) -> Result<Vec<u8>> {
        let mut copy = self.clone();
        copy.authentication_tag = None;
        copy.session_secret = None;
        canonical_json_bytes(&copy)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProtocolError {
    pub code: String,
    pub message: String,
}

impl From<&ComputerUseError> for AgentProtocolError {
    fn from(error: &ComputerUseError) -> Self {
        Self {
            code: error.code().to_owned(),
            message: error.message(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentResponse {
    pub version: u32,
    #[serde(rename = "requestIdentifier")]
    pub request_identifier: String,
    #[serde(rename = "toolResult", skip_serializing_if = "Option::is_none")]
    pub tool_result: Option<ToolExecutionResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt: Option<ActionReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pong: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AgentProtocolError>,
    #[serde(rename = "sessionIdentifier", skip_serializing_if = "Option::is_none")]
    pub session_identifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sequence: Option<u64>,
    #[serde(rename = "authenticationTag", skip_serializing_if = "Option::is_none")]
    pub authentication_tag: Option<String>,
}

impl AgentResponse {
    pub fn new(request_identifier: String) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_identifier,
            tool_result: None,
            receipt: None,
            pong: None,
            error: None,
            session_identifier: None,
            sequence: None,
            authentication_tag: None,
        }
    }

    pub fn authenticated(
        mut self,
        session_identifier: String,
        sequence: u64,
        key: &[u8],
    ) -> Result<Self> {
        self.session_identifier = Some(session_identifier);
        self.sequence = Some(sequence);
        self.authentication_tag = None;
        let tag = session_tag(key, "response-v2", &self.authentication_payload()?)?;
        self.authentication_tag = Some(tag);
        Ok(self)
    }

    pub fn authentication_payload(&self) -> Result<Vec<u8>> {
        let mut copy = self.clone();
        copy.authentication_tag = None;
        canonical_json_bytes(&copy)
    }
}

fn canonical_json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    // serde_json objects are BTreeMap-backed, so serializing through Value
    // yields sorted keys — the same canonical form as Swift's `.sortedKeys`.
    let value = serde_json::to_value(value).map_err(|_| {
        ComputerUseError::InternalFailure("Payload serialization failed.".to_owned())
    })?;
    serde_json::to_vec(&value)
        .map_err(|_| ComputerUseError::InternalFailure("Payload serialization failed.".to_owned()))
}

/// Computes the base64 HMAC-SHA256 tag over `domain || 0x00 || payload`.
pub fn session_tag(key: &[u8], domain: &str, payload: &[u8]) -> Result<String> {
    if key.len() != 32 {
        return Err(ComputerUseError::InvalidArguments(
            "Invalid app-agent session key.".to_owned(),
        ));
    }
    let mut mac = Hmac::<Sha256>::new_from_slice(key).map_err(|_| {
        ComputerUseError::InvalidArguments("Invalid app-agent session key.".to_owned())
    })?;
    mac.update(domain.as_bytes());
    mac.update(&[0]);
    mac.update(payload);
    Ok(base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes()))
}

pub fn verify_session_tag(tag: &str, key: &[u8], domain: &str, payload: &[u8]) -> bool {
    let Ok(supplied) = base64::engine::general_purpose::STANDARD.decode(tag) else {
        return false;
    };
    let Ok(expected) = session_tag(key, domain, payload) else {
        return false;
    };
    let Ok(expected) = base64::engine::general_purpose::STANDARD.decode(expected) else {
        return false;
    };
    if supplied.len() != expected.len() {
        return false;
    }
    supplied
        .iter()
        .zip(expected.iter())
        .fold(0u8, |accumulator, (left, right)| {
            accumulator | (left ^ right)
        })
        == 0
}

pub fn initialization_proof(
    key: &[u8],
    client_identifier: &str,
    request_identifier: &str,
    session_identifier: &str,
) -> Result<String> {
    session_tag(
        key,
        "initialize-v2",
        format!("{client_identifier}\u{1f}{request_identifier}\u{1f}{session_identifier}")
            .as_bytes(),
    )
}

pub fn verify_initialization_proof(
    proof: &str,
    key: &[u8],
    client_identifier: &str,
    request_identifier: &str,
    session_identifier: &str,
) -> bool {
    verify_session_tag(
        proof,
        key,
        "initialize-v2",
        format!("{client_identifier}\u{1f}{request_identifier}\u{1f}{session_identifier}")
            .as_bytes(),
    )
}

pub fn encode_frame<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let payload = canonical_json_bytes(value)?;
    if payload.len() > MAXIMUM_FRAME_BYTES {
        return Err(ComputerUseError::StateUnavailable(
            "App-agent response exceeds the wire limit.".to_owned(),
        ));
    }
    let mut frame = Vec::with_capacity(8 + payload.len());
    frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    frame.extend_from_slice(&payload);
    Ok(frame)
}

pub fn decode_length(header: &[u8]) -> Result<usize> {
    let bytes: [u8; 8] = header.try_into().map_err(|_| {
        ComputerUseError::InvalidArguments("Invalid app-agent frame header.".to_owned())
    })?;
    let length = u64::from_be_bytes(bytes);
    if length == 0 || length > MAXIMUM_FRAME_BYTES as u64 {
        return Err(ComputerUseError::InvalidArguments(
            "Invalid app-agent frame length.".to_owned(),
        ));
    }
    Ok(length as usize)
}

pub fn decode_request(data: &[u8]) -> Result<AgentRequest> {
    require_object_keys(
        data,
        &[
            "version",
            "requestIdentifier",
            "clientIdentifier",
            "kind",
            "toolName",
            "arguments",
            "actionIdentifier",
            "receiptIdentifier",
            "sessionIdentifier",
            "sequence",
            "authenticationTag",
            "sessionSecret",
        ],
        &["version", "requestIdentifier", "clientIdentifier", "kind"],
    )?;
    let request: AgentRequest = serde_json::from_slice(data).map_err(|_| {
        ComputerUseError::InvalidArguments("Invalid app-agent wire schema.".to_owned())
    })?;
    request.validate()?;
    Ok(request)
}

pub fn decode_response(data: &[u8]) -> Result<AgentResponse> {
    require_object_keys(
        data,
        &[
            "version",
            "requestIdentifier",
            "toolResult",
            "receipt",
            "pong",
            "error",
            "sessionIdentifier",
            "sequence",
            "authenticationTag",
        ],
        &["version", "requestIdentifier"],
    )?;
    let response: AgentResponse = serde_json::from_slice(data).map_err(|_| {
        ComputerUseError::InvalidArguments("Invalid app-agent wire schema.".to_owned())
    })?;
    if response.version != PROTOCOL_VERSION || !is_valid_uuid(&response.request_identifier) {
        return Err(ComputerUseError::InvalidArguments(
            "Invalid app-agent response envelope.".to_owned(),
        ));
    }
    Ok(response)
}

fn require_object_keys(data: &[u8], allowed: &[&str], required: &[&str]) -> Result<()> {
    let value: Value = serde_json::from_slice(data).map_err(|_| {
        ComputerUseError::InvalidArguments("Invalid app-agent wire schema.".to_owned())
    })?;
    let object = value.as_object().ok_or_else(|| {
        ComputerUseError::InvalidArguments("Invalid app-agent wire schema.".to_owned())
    })?;
    let keys_allowed = object.keys().all(|key| allowed.contains(&key.as_str()));
    let keys_present = required.iter().all(|key| object.contains_key(*key));
    if !keys_allowed || !keys_present {
        return Err(ComputerUseError::InvalidArguments(
            "Invalid app-agent wire schema.".to_owned(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Map;

    fn key() -> Vec<u8> {
        vec![7u8; 32]
    }

    fn base_request(kind: AgentRequestKind) -> AgentRequest {
        AgentRequest::new(Uuid::new_v4().to_string(), Uuid::new_v4().to_string(), kind)
    }

    fn authenticated(mut request: AgentRequest) -> AgentRequest {
        request.session_identifier = Some(Uuid::new_v4().to_string());
        request.sequence = Some(1);
        request.authentication_tag =
            Some(base64::engine::general_purpose::STANDARD.encode([0u8; 32]));
        request
    }

    #[test]
    fn initialize_requires_exactly_a_32_byte_secret() {
        let mut request = base_request(AgentRequestKind::Initialize);
        request.session_secret = Some(base64::engine::general_purpose::STANDARD.encode([1u8; 32]));
        request.validate().unwrap();

        request.session_secret = Some(base64::engine::general_purpose::STANDARD.encode([1u8; 16]));
        assert!(request.validate().is_err());

        let mut with_session = base_request(AgentRequestKind::Initialize);
        with_session.session_secret =
            Some(base64::engine::general_purpose::STANDARD.encode([1u8; 32]));
        with_session.session_identifier = Some(Uuid::new_v4().to_string());
        assert!(with_session.validate().is_err());
    }

    #[test]
    fn call_tool_validates_names_and_action_identifiers() {
        let mut request = authenticated(base_request(AgentRequestKind::CallTool));
        request.tool_name = Some("click".to_owned());
        request.arguments = Some(Map::new());
        assert!(
            request.validate().is_err(),
            "action tool without action id must fail"
        );

        request.action_identifier = Some("action-1".to_owned());
        request.validate().unwrap();

        request.action_identifier = Some("bad\u{7}id".to_owned());
        assert!(request.validate().is_err(), "control characters must fail");

        let mut read_only = authenticated(base_request(AgentRequestKind::CallTool));
        read_only.tool_name = Some("list_apps".to_owned());
        read_only.arguments = Some(Map::new());
        read_only.validate().unwrap();
        read_only.action_identifier = Some("action-1".to_owned());
        assert!(
            read_only.validate().is_err(),
            "read-only tool with action id must fail"
        );

        let mut unknown = authenticated(base_request(AgentRequestKind::CallTool));
        unknown.tool_name = Some("not_a_tool".to_owned());
        unknown.arguments = Some(Map::new());
        assert!(unknown.validate().is_err());
    }

    #[test]
    fn non_initialize_requires_full_session_envelope() {
        let request = base_request(AgentRequestKind::Ping);
        assert!(request.validate().is_err());
        authenticated(base_request(AgentRequestKind::Ping))
            .validate()
            .unwrap();

        let mut with_secret = authenticated(base_request(AgentRequestKind::Ping));
        with_secret.session_secret =
            Some(base64::engine::general_purpose::STANDARD.encode([1u8; 32]));
        assert!(with_secret.validate().is_err());

        let mut zero_sequence = authenticated(base_request(AgentRequestKind::Ping));
        zero_sequence.sequence = Some(0);
        assert!(zero_sequence.validate().is_err());
    }

    #[test]
    fn request_tags_round_trip_and_reject_tampering() {
        let mut request = base_request(AgentRequestKind::Ping);
        request.arguments = None;
        let session = Uuid::new_v4().to_string();
        let request = request.authenticated(session.clone(), 3, &key()).unwrap();
        let payload = request.authentication_payload().unwrap();
        assert!(verify_session_tag(
            request.authentication_tag.as_deref().unwrap(),
            &key(),
            "request-v2",
            &payload,
        ));
        // Wrong domain fails.
        assert!(!verify_session_tag(
            request.authentication_tag.as_deref().unwrap(),
            &key(),
            "response-v2",
            &payload,
        ));
        // Tampered payload fails.
        let mut tampered = request.clone();
        tampered.sequence = Some(4);
        assert!(!verify_session_tag(
            request.authentication_tag.as_deref().unwrap(),
            &key(),
            "request-v2",
            &tampered.authentication_payload().unwrap(),
        ));
        // Wrong key fails.
        assert!(!verify_session_tag(
            request.authentication_tag.as_deref().unwrap(),
            &[9u8; 32],
            "request-v2",
            &payload,
        ));
    }

    #[test]
    fn initialization_proof_round_trips() {
        let client = Uuid::new_v4().to_string();
        let request = Uuid::new_v4().to_string();
        let session = Uuid::new_v4().to_string();
        let proof = initialization_proof(&key(), &client, &request, &session).unwrap();
        assert!(verify_initialization_proof(
            &proof,
            &key(),
            &client,
            &request,
            &session
        ));
        assert!(!verify_initialization_proof(
            &proof,
            &key(),
            &client,
            &request,
            "other"
        ));
    }

    #[test]
    fn wire_frames_round_trip_and_enforce_limits() {
        let request = authenticated(base_request(AgentRequestKind::Ping));
        let frame = encode_frame(&request).unwrap();
        let length = decode_length(&frame[..8]).unwrap();
        assert_eq!(length, frame.len() - 8);
        let decoded = decode_request(&frame[8..]).unwrap();
        assert_eq!(decoded, request);

        assert!(decode_length(&0u64.to_be_bytes()).is_err());
        assert!(decode_length(&(MAXIMUM_FRAME_BYTES as u64 + 1).to_be_bytes()).is_err());
        assert!(decode_length(&frame[..4]).is_err());
    }

    #[test]
    fn wire_decoding_rejects_unknown_and_missing_keys() {
        let request = authenticated(base_request(AgentRequestKind::Ping));
        let mut value = serde_json::to_value(&request).unwrap();
        value["surprise"] = serde_json::json!(true);
        assert!(decode_request(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut missing = serde_json::to_value(&request).unwrap();
        missing.as_object_mut().unwrap().remove("kind");
        assert!(decode_request(&serde_json::to_vec(&missing).unwrap()).is_err());

        let response = AgentResponse::new(Uuid::new_v4().to_string());
        let mut value = serde_json::to_value(&response).unwrap();
        value["extra"] = serde_json::json!(1);
        assert!(decode_response(&serde_json::to_vec(&value).unwrap()).is_err());

        let mut bad_version = AgentResponse::new(Uuid::new_v4().to_string());
        bad_version.version = 1;
        assert!(decode_response(&serde_json::to_vec(&bad_version).unwrap()).is_err());
    }

    #[test]
    fn response_tags_round_trip() {
        let response = AgentResponse::new(Uuid::new_v4().to_string());
        let session = Uuid::new_v4().to_string();
        let response = response.authenticated(session, 9, &key()).unwrap();
        assert!(verify_session_tag(
            response.authentication_tag.as_deref().unwrap(),
            &key(),
            "response-v2",
            &response.authentication_payload().unwrap(),
        ));
    }
}
