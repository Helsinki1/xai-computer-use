//! The stdio MCP (JSON-RPC 2.0) server, mirroring `MCPServer.swift` exactly:
//! strict envelope validation, the reserved `xai/computer-use-v2` trusted
//! invocation metadata, and protected-carrier result mapping.

use base64::Engine as _;
use serde_json::{json, Map, Value};

use crate::catalog;
use crate::models::{JsonObject, ToolCallContext, ToolExecutionResult};

const SUPPORTED_PROTOCOL_VERSIONS: [&str; 2] = ["2025-06-18", "2025-03-26"];
const MAXIMUM_LINE_BYTES: usize = crate::protocol::MAXIMUM_FRAME_BYTES;
const MAX_OBSERVATION_TEXT_BYTES: usize = 16 * 1024;
const MAX_IMAGE_BYTES: usize = 900_000;

/// The relay-side tool transport: forwards calls to the daemon.
pub trait ToolCaller {
    fn call_tool(
        &mut self,
        name: &str,
        arguments: JsonObject,
        context: &ToolCallContext,
    ) -> ToolExecutionResult;
    fn client_disconnected(&mut self);
}

pub struct McpServer<C: ToolCaller> {
    caller: C,
    client_identifier: String,
    initialized: bool,
    server_name: &'static str,
}

struct Failure {
    id: Value,
    code: i64,
    message: &'static str,
}

impl Failure {
    fn invalid_request(id: Value) -> Self {
        Self {
            id,
            code: -32_600,
            message: "Invalid Request",
        }
    }

    fn invalid_params(id: Value) -> Self {
        Self {
            id,
            code: -32_602,
            message: "Invalid params",
        }
    }
}

type DispatchResult = std::result::Result<Option<Value>, Failure>;

impl<C: ToolCaller> McpServer<C> {
    pub fn new(caller: C, client_identifier: String, server_name: &'static str) -> Self {
        Self {
            caller,
            client_identifier,
            initialized: false,
            server_name,
        }
    }

    pub fn disconnect(&mut self) {
        self.caller.client_disconnected();
    }

    pub fn handle(&mut self, line: &str) -> Option<String> {
        if line.len() > MAXIMUM_LINE_BYTES {
            return Some(encode(error_response(
                Value::Null,
                -32_600,
                "Invalid Request",
            )));
        }
        let value: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return Some(encode(error_response(Value::Null, -32_700, "Parse error"))),
        };
        match self.dispatch(&value) {
            Ok(response) => response.map(encode),
            Err(failure) => Some(encode(error_response(
                failure.id,
                failure.code,
                failure.message,
            ))),
        }
    }

    fn dispatch(&mut self, value: &Value) -> DispatchResult {
        let request = parse_request(value)?;
        match request.method.as_str() {
            "initialize" => self.handle_initialize(&request),
            "notifications/initialized" => {
                if request.id.is_some() {
                    return Err(Failure::invalid_request(request.id.unwrap_or(Value::Null)));
                }
                require_empty_or_meta(&request.params, Value::Null)?;
                Ok(None)
            }
            "ping" => {
                let id = request
                    .id
                    .clone()
                    .ok_or_else(|| Failure::invalid_request(Value::Null))?;
                require_empty_or_meta(&request.params, id.clone())?;
                Ok(Some(success_response(id, json!({}))))
            }
            "tools/list" => {
                let id = request
                    .id
                    .clone()
                    .filter(|_| self.initialized)
                    .ok_or_else(|| {
                        Failure::invalid_request(request.id.clone().unwrap_or(Value::Null))
                    })?;
                let params = optional_object(&request.params, &id)?;
                require_allowed_keys(&params, &["cursor", "_meta"], &id)?;
                let cursor_valid = match params.get("cursor") {
                    None | Some(Value::Null) => true,
                    Some(_) => false,
                };
                if !cursor_valid {
                    return Err(Failure::invalid_params(id));
                }
                let tools: Vec<Value> = catalog::all().iter().map(|tool| tool.json()).collect();
                Ok(Some(success_response(id, json!({"tools": tools}))))
            }
            "tools/call" => self.handle_tools_call(&request),
            _ => match request.id {
                Some(id) => Err(Failure {
                    id,
                    code: -32_601,
                    message: "Method not found",
                }),
                None => Ok(None),
            },
        }
    }

    fn handle_initialize(&mut self, request: &Request) -> DispatchResult {
        let id = request
            .id
            .clone()
            .ok_or_else(|| Failure::invalid_request(Value::Null))?;
        let params = required_object(&request.params, &id)?;
        require_allowed_keys(
            &params,
            &["protocolVersion", "capabilities", "clientInfo", "_meta"],
            &id,
        )?;
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .ok_or_else(|| Failure::invalid_params(id.clone()))?;
        if !params.get("capabilities").is_some_and(Value::is_object)
            || !params.get("clientInfo").is_some_and(Value::is_object)
        {
            return Err(Failure::invalid_params(id));
        }
        let selected = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
            requested
        } else {
            SUPPORTED_PROTOCOL_VERSIONS[0]
        };
        self.initialized = true;
        Ok(Some(success_response(
            id,
            json!({
                "protocolVersion": selected,
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": self.server_name, "version": "0.1.0"},
            }),
        )))
    }

    fn handle_tools_call(&mut self, request: &Request) -> DispatchResult {
        let id = request
            .id
            .clone()
            .filter(|_| self.initialized)
            .ok_or_else(|| Failure::invalid_request(request.id.clone().unwrap_or(Value::Null)))?;
        let params = required_object(&request.params, &id)?;
        require_allowed_keys(&params, &["name", "arguments", "_meta"], &id)?;
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| catalog::is_accepted_tool(name))
            .ok_or_else(|| Failure::invalid_params(id.clone()))?
            .to_owned();
        let arguments = match params.get("arguments") {
            None => Map::new(),
            Some(Value::Object(object)) => object.clone(),
            Some(_) => return Err(Failure::invalid_params(id)),
        };
        let action_identifier = parse_trusted_invocation(params.get("_meta"), &name, &id)?;
        let action_identifier = catalog::is_action_tool(&name).then_some(action_identifier);
        let context = ToolCallContext {
            client_identifier: self.client_identifier.clone(),
            action_identifier,
        };
        let result = self.caller.call_tool(&name, arguments, &context);
        Ok(Some(success_response(id, tool_call_result(&result))))
    }
}

struct Request {
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

fn valid_id(value: &Value) -> bool {
    matches!(value, Value::String(_)) || value.as_i64().is_some() || value.as_u64().is_some()
}

fn parse_request(value: &Value) -> std::result::Result<Request, Failure> {
    let object = value
        .as_object()
        .ok_or_else(|| Failure::invalid_request(Value::Null))?;
    let raw_id = object.get("id");
    let id = raw_id.filter(|id| valid_id(id)).cloned();
    let keys_valid = object
        .keys()
        .all(|key| ["jsonrpc", "id", "method", "params"].contains(&key.as_str()));
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !keys_valid
        || object.get("jsonrpc") != Some(&Value::String("2.0".to_owned()))
        || method.is_empty()
        || (raw_id.is_some() && id.is_none())
    {
        return Err(Failure::invalid_request(id.unwrap_or(Value::Null)));
    }
    if let Some(params) = object.get("params") {
        if !params.is_object() && !params.is_array() {
            return Err(Failure::invalid_params(id.unwrap_or(Value::Null)));
        }
    }
    Ok(Request {
        id,
        method: method.to_owned(),
        params: object.get("params").cloned(),
    })
}

fn required_object(params: &Option<Value>, id: &Value) -> std::result::Result<JsonObject, Failure> {
    params
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| Failure::invalid_params(id.clone()))
}

fn optional_object(params: &Option<Value>, id: &Value) -> std::result::Result<JsonObject, Failure> {
    match params {
        None => Ok(Map::new()),
        Some(value) => value
            .as_object()
            .cloned()
            .ok_or_else(|| Failure::invalid_params(id.clone())),
    }
}

fn require_allowed_keys(
    object: &JsonObject,
    allowed: &[&str],
    id: &Value,
) -> std::result::Result<(), Failure> {
    if object.keys().all(|key| allowed.contains(&key.as_str())) {
        Ok(())
    } else {
        Err(Failure::invalid_params(id.clone()))
    }
}

fn require_empty_or_meta(params: &Option<Value>, id: Value) -> std::result::Result<(), Failure> {
    let object = optional_object(params, &id)?;
    require_allowed_keys(&object, &["_meta"], &id)
}

fn valid_trusted_identity(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

/// Validates the reserved `xai/computer-use-v2` invocation metadata and
/// returns the action identifier.
fn parse_trusted_invocation(
    value: Option<&Value>,
    tool_name: &str,
    id: &Value,
) -> std::result::Result<String, Failure> {
    let invalid = || Failure::invalid_params(id.clone());
    let root = value.and_then(Value::as_object).ok_or_else(invalid)?;
    if root.len() != 1 || !root.contains_key("xai/computer-use-v2") {
        return Err(invalid());
    }
    let payload = root
        .get("xai/computer-use-v2")
        .and_then(Value::as_object)
        .ok_or_else(invalid)?;
    let expected_keys = [
        "profile",
        "logical_call_id",
        "session_id",
        "workflow_id",
        "action_id",
        "tool_name",
    ];
    if payload.len() != expected_keys.len()
        || !expected_keys.iter().all(|key| payload.contains_key(*key))
    {
        return Err(invalid());
    }
    if payload.get("profile").and_then(Value::as_str) != Some("computer-use-v2")
        || payload.get("tool_name").and_then(Value::as_str) != Some(tool_name)
    {
        return Err(invalid());
    }
    let identifiers: Vec<&str> = ["logical_call_id", "session_id", "workflow_id", "action_id"]
        .iter()
        .filter_map(|key| payload.get(*key).and_then(Value::as_str))
        .collect();
    if identifiers.len() != 4
        || !identifiers
            .iter()
            .all(|value| valid_trusted_identity(value))
    {
        return Err(invalid());
    }
    Ok(identifiers[3].to_owned())
}

fn tool_call_result(result: &ToolExecutionResult) -> Value {
    if let Some(carrier) = &result.protected_carrier {
        let image = result.image_png.as_ref();
        let valid = image.is_some_and(|image| !image.is_empty() && image.len() <= MAX_IMAGE_BYTES)
            && result.text.len() <= MAX_OBSERVATION_TEXT_BYTES
            && !result.is_error
            && result.structured_content.is_none();
        if !valid {
            return json!({
                "content": [{"type": "text", "text": "Protected computer-use observation rejected by the relay."}],
                "isError": true,
            });
        }
        let image =
            base64::engine::general_purpose::STANDARD.encode(image.expect("validated above"));
        return json!({
            "content": [
                {"type": "text", "text": result.text},
                {"type": "image", "mimeType": "image/png", "data": image},
            ],
            "isError": false,
            "_meta": {"xai/computer-use-v2": carrier.observation_json()},
        });
    }
    if result.image_png.is_some() {
        return json!({
            "content": [{"type": "text", "text": "Unattested computer-use image rejected by the relay."}],
            "isError": true,
        });
    }
    let mut response = json!({
        "content": [{"type": "text", "text": result.text}],
        "isError": result.is_error,
    });
    if let Some(structured) = &result.structured_content {
        response["structuredContent"] = structured.clone();
    }
    response
}

fn success_response(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn encode(value: Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|_| {
        "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"Internal error\"}}"
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        AppTarget, CapturedDesktopState, GlobalScreenRect, ProtectedComputerUseCarrier,
        SnapshotEnvelope, WindowGeometry,
    };

    struct RecordingCaller {
        last: Option<(String, JsonObject, ToolCallContext)>,
        result: ToolExecutionResult,
        disconnected: bool,
    }

    impl RecordingCaller {
        fn new(result: ToolExecutionResult) -> Self {
            Self {
                last: None,
                result,
                disconnected: false,
            }
        }
    }

    impl ToolCaller for RecordingCaller {
        fn call_tool(
            &mut self,
            name: &str,
            arguments: JsonObject,
            context: &ToolCallContext,
        ) -> ToolExecutionResult {
            self.last = Some((name.to_owned(), arguments, context.clone()));
            self.result.clone()
        }

        fn client_disconnected(&mut self) {
            self.disconnected = true;
        }
    }

    fn server(result: ToolExecutionResult) -> McpServer<RecordingCaller> {
        let mut server = McpServer::new(
            RecordingCaller::new(result),
            "client-1".to_owned(),
            "grok-computer-use-linux",
        );
        let response = server
            .handle(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{}}}"#)
            .unwrap();
        assert!(response.contains("2025-06-18"));
        server
    }

    fn meta(tool: &str) -> String {
        format!(
            r#"{{"xai/computer-use-v2":{{"profile":"computer-use-v2","logical_call_id":"l-1","session_id":"s-1","workflow_id":"w-1","action_id":"a-1","tool_name":"{tool}"}}}}"#
        )
    }

    #[test]
    fn tools_list_requires_initialization_and_serves_the_catalog() {
        let mut uninitialized = McpServer::new(
            RecordingCaller::new(ToolExecutionResult::text("ok")),
            "client-1".to_owned(),
            "grok-computer-use-linux",
        );
        let refused = uninitialized
            .handle(r#"{"jsonrpc":"2.0","id":5,"method":"tools/list"}"#)
            .unwrap();
        assert!(refused.contains("-32600"));

        let mut server = server(ToolExecutionResult::text("ok"));
        let listing = server
            .handle(r#"{"jsonrpc":"2.0","id":6,"method":"tools/list"}"#)
            .unwrap();
        for tool in catalog::all() {
            assert!(
                listing.contains(tool.name),
                "catalog is missing {}",
                tool.name
            );
        }
        for hidden in catalog::HIDDEN_TOOL_NAMES {
            assert!(!listing.contains(&format!("\"name\":\"{hidden}\"")));
        }
    }

    #[test]
    fn tools_call_requires_trusted_invocation_metadata() {
        let mut server = server(ToolExecutionResult::text("ok"));
        let missing_meta = server
            .handle(r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"list_apps","arguments":{}}}"#)
            .unwrap();
        assert!(missing_meta.contains("-32602"));

        let request = format!(
            r#"{{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{{"name":"list_apps","arguments":{{}},"_meta":{}}}}}"#,
            meta("list_apps")
        );
        let accepted = server.handle(&request).unwrap();
        assert!(accepted.contains("\"isError\":false"));
        let (name, _, context) = server.caller.last.clone().unwrap();
        assert_eq!(name, "list_apps");
        // Read-only tools carry no action identifier.
        assert_eq!(context.action_identifier, None);
    }

    #[test]
    fn action_tools_receive_the_trusted_action_identifier() {
        let mut server = server(ToolExecutionResult::text("ok"));
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{{"name":"click","arguments":{{"snapshot_id":"0000000000000000","target":{{"kind":"pixel","x_px":1,"y_px":1}}}},"_meta":{}}}}}"#,
            meta("click")
        );
        server.handle(&request).unwrap();
        let (_, _, context) = server.caller.last.clone().unwrap();
        assert_eq!(context.action_identifier.as_deref(), Some("a-1"));
    }

    #[test]
    fn wrong_tool_name_in_meta_is_rejected() {
        let mut server = server(ToolExecutionResult::text("ok"));
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{{"name":"click","arguments":{{}},"_meta":{}}}}}"#,
            meta("type_text")
        );
        let rejected = server.handle(&request).unwrap();
        assert!(rejected.contains("-32602"));
    }

    #[test]
    fn unattested_images_are_rejected_and_carriers_pass_through() {
        let mut leaky = ToolExecutionResult::text("observation");
        leaky.image_png = Some(vec![1, 2, 3]);
        let mut leaky_server = server(leaky);
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{{"name":"get_app_state","arguments":{{"bundle_id":"org.fixture"}},"_meta":{}}}}}"#,
            meta("get_app_state")
        );
        let rejected = leaky_server.handle(&request).unwrap();
        assert!(rejected.contains("Unattested computer-use image rejected"));

        let envelope = SnapshotEnvelope {
            snapshot_identifier: "0123456789abcdef".to_owned(),
            delivery_attestation_identifier: "fedcba9876543210".to_owned(),
            captured: CapturedDesktopState {
                app: AppTarget {
                    name: "Fixture".to_owned(),
                    bundle_identifier: Some("org.fixture".to_owned()),
                    process_identifier: 42,
                },
                window_title: None,
                geometry: WindowGeometry {
                    window_identifier: 7,
                    global_bounds_points: GlobalScreenRect {
                        x: 0.0,
                        y: 0.0,
                        width: 100.0,
                        height: 50.0,
                    },
                    png_width_pixels: 200,
                    png_height_pixels: 100,
                },
                screenshot_png: vec![0x89, 0x50],
                screenshot_sha256: "a".repeat(64),
                accessibility_tree: "tree".to_owned(),
                elements: Vec::new(),
            },
            lease_expires_at: time::OffsetDateTime::UNIX_EPOCH,
        };
        let carrier = ProtectedComputerUseCarrier::new(&envelope).unwrap();
        let mut protected = ToolExecutionResult::text("observation");
        protected.image_png = Some(vec![0x89, 0x50]);
        protected.protected_carrier = Some(carrier);
        let mut server = server(protected);
        let request = format!(
            r#"{{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{{"name":"get_app_state","arguments":{{"bundle_id":"org.fixture"}},"_meta":{}}}}}"#,
            meta("get_app_state")
        );
        let accepted = server.handle(&request).unwrap();
        assert!(accepted.contains("xai/computer-use-v2"));
        assert!(accepted.contains("png_sha256"));
        assert!(accepted.contains("\"isError\":false"));
    }

    #[test]
    fn malformed_envelopes_fail_with_jsonrpc_errors() {
        let mut server = server(ToolExecutionResult::text("ok"));
        assert!(server.handle("not json").unwrap().contains("-32700"));
        assert!(server
            .handle(r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#)
            .unwrap()
            .contains("-32600"));
        assert!(server
            .handle(r#"{"jsonrpc":"2.0","id":1,"method":"nope"}"#)
            .unwrap()
            .contains("-32601"));
        // Notifications with unknown methods are silently dropped.
        assert!(server
            .handle(r#"{"jsonrpc":"2.0","method":"nope"}"#)
            .is_none());
    }
}
