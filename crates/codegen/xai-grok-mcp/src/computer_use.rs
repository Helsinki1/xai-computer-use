//! Trusted MCP profile for native computer use.
//!
//! This module is deliberately separate from generic MCP.  A server name,
//! tool metadata, or model-supplied argument can never opt into this path:
//! only a [`TrustedMcpServerSpec`] minted by [`crate::servers::McpState`]
//! carries the in-process capability used by the trusted client.

use std::collections::{BTreeMap, HashMap};
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use parking_lot::Mutex;
use rmcp::model::{CallToolResult, ContentBlock, Meta, Tool};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

/// Reserved MCP namespace. Generic config paths must reject it case-insensitively.
pub const COMPUTER_USE_MCP_SERVER_NAME: &str = "xai_computer_use";
/// Stable, non-configurable name of the trusted contract.
pub const COMPUTER_USE_V2_PROFILE_NAME: &str = "computer-use-v2";
/// Protocol `_meta` key used in both calls and snapshot-bearing results.
pub const COMPUTER_USE_V2_META_KEY: &str = "xai/computer-use-v2";
/// Reserved macOS suffix enforced by the MCP capability constructor. The shell
/// additionally verifies this bundle/path has no symlink components and a
/// valid platform trust decision.
pub const MACOS_TRUSTED_RELAY_PATH_SUFFIX: &str =
    "Grok Computer Use.app/Contents/MacOS/grok-computer-use-mcp";
/// Reserved Linux install suffix enforced by the MCP capability constructor.
pub const LINUX_TRUSTED_RELAY_PATH_SUFFIX: &str =
    ".local/libexec/grok-computer-use/grok-computer-use-mcp";

pub fn trusted_relay_path_matches(path: &Path) -> bool {
    path.is_absolute()
        && (path.ends_with(MACOS_TRUSTED_RELAY_PATH_SUFFIX)
            || path.ends_with(LINUX_TRUSTED_RELAY_PATH_SUFFIX))
}
/// Hidden relay method called only after final inference-body attestation succeeds.
pub const ATTEST_SNAPSHOT_DELIVERY_TOOL: &str = "attest_snapshot_delivery";
/// Hidden relay method used to invalidate all leases for a session.
pub const INVALIDATE_SESSION_TOOL: &str = "invalidate_session";
/// Hidden relay method used while a protected inference is still in flight.
pub const LEASE_HEARTBEAT_TOOL: &str = "lease_heartbeat";
/// Hidden relay method used to release a completed operation receipt.
pub const RELEASE_OPERATION_TOOL: &str = "release_operation";

pub const MAX_PNG_BYTES: usize = 900_000;
pub const MAX_PNG_SIDE: u32 = 1_280;
pub const MAX_PNG_PIXELS: u64 = 1_638_400;
pub const MAX_OBSERVATION_BYTES: usize = 16 * 1024;
pub const COMPUTER_USE_OBSERVATION_PLACEHOLDER: &str =
    "[trusted computer-use observation retained for protected inference]";

const MIN_SNAPSHOT_ID_BYTES: usize = 16;
const MAX_SNAPSHOT_ID_BYTES: usize = 128;
const MAX_IDENTITY_BYTES: usize = 256;
const DEFAULT_OBSERVATION_TTL: Duration = Duration::from_secs(120);
const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

/// Profiles are selected only by trusted Rust host wiring. This type has no
/// serde implementation and is never inferred from an MCP/config name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum TrustedMcpProfile {
    ComputerUseV2,
}

/// Moveable capability describing the one trusted relay to start.
///
/// Fields are private and there is no public constructor. Ordinary MCP config
/// can therefore describe the same executable/name but cannot produce the
/// capability checked by the client and tool-dispatch paths.
#[derive(Clone)]
pub struct TrustedMcpServerSpec {
    pub(crate) profile: TrustedMcpProfile,
    pub(crate) relay_path: PathBuf,
    pub(crate) observations: Arc<ComputerUseObservationCarrier>,
}

impl TrustedMcpServerSpec {
    pub(crate) fn new(profile: TrustedMcpProfile, relay_path: PathBuf) -> Self {
        Self {
            profile,
            relay_path,
            observations: Arc::new(ComputerUseObservationCarrier::default()),
        }
    }

    pub fn relay_path(&self) -> &Path {
        &self.relay_path
    }
}

/// Trusted per-invocation identity supplied through `ToolCallContext`.
///
/// The runtime call id remains the logical call id; this extension binds that
/// id to the host session, workflow/turn, and durable action identity. It has
/// no serde or `Debug` implementation so it cannot accidentally become model
/// input or ordinary telemetry.
pub struct ComputerUseInvocationContext {
    session_id: Box<str>,
    workflow_id: Box<str>,
    action_id: Box<str>,
}

impl ComputerUseInvocationContext {
    pub fn new(
        session_id: impl Into<String>,
        workflow_id: impl Into<String>,
        action_id: impl Into<String>,
    ) -> Result<Self, ComputerUseContractError> {
        let session_id = session_id.into();
        let workflow_id = workflow_id.into();
        let action_id = action_id.into();
        validate_identity(&session_id)?;
        validate_identity(&workflow_id)?;
        validate_identity(&action_id)?;
        Ok(Self {
            session_id: session_id.into_boxed_str(),
            workflow_id: workflow_id.into_boxed_str(),
            action_id: action_id.into_boxed_str(),
        })
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    pub fn action_id(&self) -> &str {
        &self.action_id
    }

    pub(crate) fn scope_key(&self) -> String {
        format!("{}\u{1f}{}", self.session_id, self.workflow_id)
    }

    pub(crate) fn request_meta(
        &self,
        logical_call_id: &str,
        tool_name: &str,
    ) -> Result<Meta, ComputerUseContractError> {
        validate_identity(logical_call_id)?;
        let mut root = Map::new();
        root.insert(
            COMPUTER_USE_V2_META_KEY.to_string(),
            json!({
                "profile": COMPUTER_USE_V2_PROFILE_NAME,
                "logical_call_id": logical_call_id,
                "session_id": self.session_id(),
                "workflow_id": self.workflow_id(),
                "action_id": self.action_id(),
                "tool_name": tool_name,
            }),
        );
        Ok(Meta(root))
    }
}

/// Shell-facing binding point. Call this after the runtime call id is final
/// and immediately before dispatching a trusted computer-use tool. The action
/// id is the same durable, model-visible tool-call id; callers cannot
/// accidentally bind metadata to a different invocation.
pub fn bind_invocation_context(
    ctx: &mut xai_tool_runtime::ToolCallContext,
    session_id: impl Into<String>,
    workflow_id: impl Into<String>,
) -> Result<(), ComputerUseContractError> {
    let action_id = ctx.call_id.to_string();
    ctx.insert(ComputerUseInvocationContext::new(
        session_id,
        workflow_id,
        action_id,
    )?);
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComputerUseToolClass {
    Observation,
    /// Reads a delivered snapshot as a planning hint and returns a fresh,
    /// protected visual observation. It must retain the supplied snapshot
    /// until the native tool has consumed that hint.
    Planning,
    Effectful,
}

pub fn classify_tool(name: &str) -> Option<ComputerUseToolClass> {
    match name {
        "list_apps" | "get_app_state" => Some(ComputerUseToolClass::Observation),
        "plan_click" => Some(ComputerUseToolClass::Planning),
        "click"
        | "drag"
        | "perform_secondary_action"
        | "scroll"
        | "type_text"
        | "press_key"
        | "set_value" => Some(ComputerUseToolClass::Effectful),
        _ => None,
    }
}

/// Decide whether a successful trusted result must be removed from ordinary
/// tool output and captured for protected inference.
///
/// `get_app_state` always owes a protected observation. Effectful tools may
/// return either a refreshed observation or a bounded text-only recovery
/// outcome, so their protected metadata is the discriminator. `list_apps`
/// never carries a screenshot.
pub(crate) fn should_capture_observation(
    tool_name: &str,
    class: ComputerUseToolClass,
    result: &CallToolResult,
) -> bool {
    if result.is_error != Some(false) {
        return false;
    }
    (tool_name == "get_app_state" || tool_name == "plan_click")
        || (class == ComputerUseToolClass::Effectful
            && result
                .meta
                .as_ref()
                .is_some_and(|meta| meta.0.contains_key(COMPUTER_USE_V2_META_KEY)))
}

pub fn is_reserved_server_name(name: &str) -> bool {
    name.eq_ignore_ascii_case(COMPUTER_USE_MCP_SERVER_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_relay_path_matcher_accepts_reserved_macos_and_linux_paths_only() {
        assert!(trusted_relay_path_matches(Path::new(
            "/Users/test/Applications/Grok Computer Use.app/Contents/MacOS/grok-computer-use-mcp"
        )));
        assert!(trusted_relay_path_matches(Path::new(
            "/home/test/.local/libexec/grok-computer-use/grok-computer-use-mcp"
        )));
        assert!(!trusted_relay_path_matches(Path::new(
            "/tmp/grok-computer-use-mcp"
        )));
        assert!(!trusted_relay_path_matches(Path::new(
            "relative/grok-computer-use-mcp"
        )));
    }
}

/// Fail-closed contract errors contain no server-provided values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[non_exhaustive]
pub enum ComputerUseContractError {
    #[error("trusted computer-use identity is invalid")]
    InvalidIdentity,
    #[error("trusted computer-use tool catalog does not match the v2 contract")]
    ToolCatalogMismatch,
    #[error("trusted computer-use tool schema does not match the v2 contract")]
    ToolSchemaMismatch,
    #[error("trusted computer-use result has an unexpected content shape")]
    UnexpectedContent,
    #[error("trusted computer-use snapshot metadata is missing or malformed")]
    InvalidMetadata,
    #[error("trusted computer-use observation exceeds the text limit")]
    ObservationTooLarge,
    #[error("trusted computer-use image is not canonical PNG base64")]
    InvalidImageEncoding,
    #[error("trusted computer-use PNG exceeds the byte limit")]
    ImageTooLarge,
    #[error("trusted computer-use PNG container is invalid")]
    InvalidPng,
    #[error("trusted computer-use PNG dimensions are invalid")]
    InvalidDimensions,
    #[error("trusted computer-use metadata dimensions do not match the PNG")]
    DimensionMismatch,
    #[error("trusted computer-use metadata hash does not match the PNG")]
    HashMismatch,
    #[error("trusted computer-use observation is unavailable for the exact call")]
    ObservationUnavailable,
    #[error("trusted computer-use already has a pending inference handoff")]
    HandoffAlreadyPending,
}

fn validate_identity(value: &str) -> Result<(), ComputerUseContractError> {
    if value.is_empty() || value.len() > MAX_IDENTITY_BYTES || value.chars().any(char::is_control) {
        Err(ComputerUseContractError::InvalidIdentity)
    } else {
        Ok(())
    }
}

fn strict_object(properties: Value, required: &[&str]) -> Value {
    let mut schema = json!({
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

fn snapshot_id_schema() -> Value {
    json!({"type":"string", "minLength":16, "maxLength":128})
}

fn bundle_id_schema() -> Value {
    json!({"type":"string", "minLength":3, "maxLength":255})
}

fn window_id_schema() -> Value {
    json!({"type":"integer", "minimum":1, "maximum":4294967295_u64})
}

fn element_id_schema() -> Value {
    json!({"type":"string", "minLength":1, "maxLength":128})
}

fn pixel_point_schema() -> Value {
    strict_object(
        json!({"x_px":{"type":"number"}, "y_px":{"type":"number"}}),
        &["x_px", "y_px"],
    )
}

/// Canonical, exact input schemas for the ten public tools.
pub fn computer_use_v2_tool_schemas() -> BTreeMap<&'static str, Value> {
    let element_target = strict_object(
        json!({
            "kind":{"const":"element"},
            "element_id":element_id_schema(),
        }),
        &["kind", "element_id"],
    );
    let pixel_target = strict_object(
        json!({
            "kind":{"const":"pixel"},
            "x_px":{"type":"number"},
            "y_px":{"type":"number"},
        }),
        &["kind", "x_px", "y_px"],
    );
    let click_target = json!({"oneOf":[element_target, pixel_target]});

    BTreeMap::from([
        ("list_apps", strict_object(json!({}), &[])),
        (
            "get_app_state",
            strict_object(
                json!({"bundle_id":bundle_id_schema(), "window_id":window_id_schema()}),
                &["bundle_id"],
            ),
        ),
        (
            "plan_click",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "target":click_target.clone(),
                    "button":{"type":"string", "enum":["left", "right"]},
                    "count":{"type":"integer", "enum":[1, 2]},
                }),
                &["snapshot_id", "target"],
            ),
        ),
        (
            "click",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "target":click_target,
                    "button":{"type":"string", "enum":["left", "right"]},
                    "count":{"type":"integer", "enum":[1, 2]},
                }),
                &["snapshot_id", "target"],
            ),
        ),
        (
            "drag",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "from":pixel_point_schema(),
                    "to":pixel_point_schema(),
                }),
                &["snapshot_id", "from", "to"],
            ),
        ),
        (
            "perform_secondary_action",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "element_id":element_id_schema(),
                    "action_id":{"type":"string", "minLength":1, "maxLength":128},
                }),
                &["snapshot_id", "element_id", "action_id"],
            ),
        ),
        (
            "scroll",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "element_id":element_id_schema(),
                    "direction":{"type":"string", "enum":["up", "down", "left", "right"]},
                    "pages":{"type":"number", "exclusiveMinimum":0, "maximum":10},
                }),
                &["snapshot_id", "element_id", "direction"],
            ),
        ),
        (
            "type_text",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "text":{"type":"string", "maxLength":32768},
                }),
                &["snapshot_id", "text"],
            ),
        ),
        (
            "press_key",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "key":{"type":"string"},
                    "modifiers":{
                        "type":"array",
                        "items":{"type":"string", "enum":["command", "control", "option", "shift", "fn"]},
                        "uniqueItems":true,
                    },
                }),
                &["snapshot_id", "key"],
            ),
        ),
        (
            "set_value",
            strict_object(
                json!({
                    "snapshot_id":snapshot_id_schema(),
                    "element_id":element_id_schema(),
                    "value":{"type":"string", "maxLength":32768},
                }),
                &["snapshot_id", "element_id", "value"],
            ),
        ),
    ])
}

pub(crate) fn verify_tool_catalog(tools: &[Tool]) -> Result<(), ComputerUseContractError> {
    let expected = computer_use_v2_tool_schemas();
    if tools.len() != expected.len() {
        return Err(ComputerUseContractError::ToolCatalogMismatch);
    }
    let mut seen = std::collections::HashSet::with_capacity(tools.len());
    for tool in tools {
        let name = tool.name.as_ref();
        if !seen.insert(name.to_string()) {
            return Err(ComputerUseContractError::ToolCatalogMismatch);
        }
        let Some(expected_schema) = expected.get(name) else {
            return Err(ComputerUseContractError::ToolCatalogMismatch);
        };
        let actual_schema = Value::Object(tool.input_schema.as_ref().clone());
        if &actual_schema != expected_schema || tool.output_schema.is_some() {
            return Err(ComputerUseContractError::ToolSchemaMismatch);
        }
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotMetadata {
    profile: String,
    snapshot_id: String,
    attestation_id: String,
    bundle_id: String,
    window_id: u64,
    png_sha256: String,
    png_width_px: u32,
    png_height_px: u32,
    capture_origin_x: f64,
    capture_origin_y: f64,
    capture_width_points: f64,
    capture_height_points: f64,
}

/// Authoritative mapping from PNG edge-space into global display points.
///
/// The relay applies:
/// `global = capture_origin + pixel * capture_points / png_pixels`.
/// Pixel inputs are valid only in the half-open PNG bounds.
#[derive(Clone, Copy)]
pub struct ComputerUseCaptureGeometry {
    origin_x: f64,
    origin_y: f64,
    width_points: f64,
    height_points: f64,
    png_width: u32,
    png_height: u32,
}

impl ComputerUseCaptureGeometry {
    pub fn capture_origin(&self) -> (f64, f64) {
        (self.origin_x, self.origin_y)
    }

    pub fn capture_size_points(&self) -> (f64, f64) {
        (self.width_points, self.height_points)
    }

    pub fn png_size_pixels(&self) -> (u32, u32) {
        (self.png_width, self.png_height)
    }

    pub fn contains_pixel_edge_coordinate(&self, x_px: f64, y_px: f64) -> bool {
        x_px.is_finite()
            && y_px.is_finite()
            && x_px >= 0.0
            && y_px >= 0.0
            && x_px < f64::from(self.png_width)
            && y_px < f64::from(self.png_height)
    }

    pub fn pixel_to_global(&self, x_px: f64, y_px: f64) -> Option<(f64, f64)> {
        self.contains_pixel_edge_coordinate(x_px, y_px).then(|| {
            (
                self.origin_x + x_px * self.width_points / f64::from(self.png_width),
                self.origin_y + y_px * self.height_points / f64::from(self.png_height),
            )
        })
    }
}

/// A validated observation retained outside ordinary `ToolOutput`.
///
/// This type intentionally implements neither `Clone`, `Debug`, nor serde.
/// `into_protected_parts` is the only path that moves its PNG/model text into
/// the sampler's request-only overlay.
pub struct ComputerUseObservation {
    logical_call_id: Box<str>,
    snapshot_id: Box<str>,
    attestation_id: Box<str>,
    bundle_id: Box<str>,
    window_id: u32,
    png_sha256: Box<str>,
    geometry: ComputerUseCaptureGeometry,
    observation: Vec<u8>,
    png: Vec<u8>,
}

/// Opaque values needed for the hidden delivery acknowledgement after the
/// sampler proves the exact PNG reached the final request body.
///
/// No `Clone`, `Debug`, or serde implementation: keep it request-local and
/// drop it if the sampler returns `NotAttached`.
pub struct ComputerUseDeliveryAttestation {
    snapshot_id: Box<str>,
    attestation_id: Box<str>,
    png_sha256: Box<str>,
}

impl ComputerUseDeliveryAttestation {
    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    pub fn attestation_id(&self) -> &str {
        &self.attestation_id
    }

    pub fn png_sha256(&self) -> &str {
        &self.png_sha256
    }
}

/// A fixed hidden operation accepted only by a capability-bearing
/// computer-use client.
///
/// The enum owns its arguments and deliberately has no serde or `Debug`
/// implementation, so generic MCP paths cannot synthesize one from a tool
/// name supplied by config or the model.
pub enum ComputerUseLifecycleRequest {
    AttestSnapshotDelivery(ComputerUseDeliveryAttestation),
    LeaseHeartbeat { snapshot_id: String },
    ReleaseOperation { snapshot_id: String },
    InvalidateSession,
}

impl ComputerUseLifecycleRequest {
    pub(crate) fn tool_name(&self) -> &'static str {
        match self {
            Self::AttestSnapshotDelivery(_) => ATTEST_SNAPSHOT_DELIVERY_TOOL,
            Self::LeaseHeartbeat { .. } => LEASE_HEARTBEAT_TOOL,
            Self::ReleaseOperation { .. } => RELEASE_OPERATION_TOOL,
            Self::InvalidateSession => INVALIDATE_SESSION_TOOL,
        }
    }

    pub(crate) fn expected_success_text(&self) -> &'static str {
        match self {
            Self::AttestSnapshotDelivery(_) => "Snapshot delivery attested.",
            Self::LeaseHeartbeat { .. } => "Desktop lease renewed.",
            Self::ReleaseOperation { .. } => "Desktop operation released.",
            Self::InvalidateSession => "Computer-use session invalidated.",
        }
    }

    pub(crate) fn into_arguments(self) -> Map<String, Value> {
        match self {
            Self::AttestSnapshotDelivery(attestation) => Map::from_iter([
                (
                    "snapshot_id".to_string(),
                    Value::String(attestation.snapshot_id.into_string()),
                ),
                (
                    "attestation_id".to_string(),
                    Value::String(attestation.attestation_id.into_string()),
                ),
                (
                    "png_sha256".to_string(),
                    Value::String(attestation.png_sha256.into_string()),
                ),
            ]),
            Self::LeaseHeartbeat { snapshot_id } | Self::ReleaseOperation { snapshot_id } => {
                Map::from_iter([("snapshot_id".to_string(), Value::String(snapshot_id))])
            }
            Self::InvalidateSession => Map::new(),
        }
    }
}

impl ComputerUseObservation {
    pub fn logical_call_id(&self) -> &str {
        &self.logical_call_id
    }

    pub fn snapshot_id(&self) -> &str {
        &self.snapshot_id
    }

    pub fn attestation_id(&self) -> &str {
        &self.attestation_id
    }

    pub fn bundle_id(&self) -> &str {
        &self.bundle_id
    }

    pub fn window_id(&self) -> u32 {
        self.window_id
    }

    pub fn png_sha256(&self) -> &str {
        &self.png_sha256
    }

    pub fn geometry(&self) -> ComputerUseCaptureGeometry {
        self.geometry
    }

    pub fn observation(&self) -> &str {
        std::str::from_utf8(&self.observation).expect("observation originated as UTF-8")
    }

    pub fn png(&self) -> &[u8] {
        &self.png
    }

    pub fn delivery_attestation(&self) -> ComputerUseDeliveryAttestation {
        ComputerUseDeliveryAttestation {
            snapshot_id: self.snapshot_id.clone(),
            attestation_id: self.attestation_id.clone(),
            png_sha256: self.png_sha256.clone(),
        }
    }

    /// Move the exact arguments expected by
    /// `SamplerHandle::attest_protected_overlay`.
    pub fn into_protected_parts(mut self) -> (String, String, Vec<u8>, String, u32, u32) {
        let snapshot_id = std::mem::take(&mut self.snapshot_id).into_string();
        let observation = String::from_utf8(std::mem::take(&mut self.observation))
            .expect("observation originated as UTF-8");
        let png = std::mem::take(&mut self.png);
        let hash = std::mem::take(&mut self.png_sha256).into_string();
        (
            snapshot_id,
            observation,
            png,
            hash,
            self.geometry.png_width,
            self.geometry.png_height,
        )
    }
}

/// Exact, one-shot bridge from a completed trusted tool call to the next
/// protected inference.
///
/// It owns the observation drained for one logical call, so no "latest
/// result" lookup is possible after this value is created. This type
/// intentionally implements neither `Clone`, `Debug`, nor serde.
pub struct ComputerUseObservationHandoff {
    workflow_id: Box<str>,
    observation: ComputerUseObservation,
}

impl ComputerUseObservationHandoff {
    pub(crate) fn new(
        workflow_id: String,
        observation: ComputerUseObservation,
    ) -> Result<Self, ComputerUseContractError> {
        validate_identity(&workflow_id)?;
        Ok(Self {
            workflow_id: workflow_id.into_boxed_str(),
            observation,
        })
    }

    pub fn logical_call_id(&self) -> &str {
        self.observation.logical_call_id()
    }

    pub fn workflow_id(&self) -> &str {
        &self.workflow_id
    }

    pub fn observation(&self) -> &ComputerUseObservation {
        &self.observation
    }

    pub fn into_observation(self) -> ComputerUseObservation {
        self.observation
    }
}

impl Drop for ComputerUseObservation {
    fn drop(&mut self) {
        self.observation.fill(0);
        self.png.fill(0);
    }
}

struct PendingObservation {
    scope_key: String,
    expires_at: Instant,
    observation: ComputerUseObservation,
}

#[derive(Default)]
struct CarrierState {
    by_call_id: HashMap<String, PendingObservation>,
    call_id_by_scope: HashMap<String, String>,
}

/// In-memory, one-shot observation handoff.
///
/// One pending observation is retained per session/workflow scope. Newer
/// observations invalidate older ones in that scope. The carrier has no
/// formatting/serialization implementation and never writes to disk.
pub struct ComputerUseObservationCarrier {
    state: Mutex<CarrierState>,
    ttl: Duration,
}

impl Default for ComputerUseObservationCarrier {
    fn default() -> Self {
        Self {
            state: Mutex::new(CarrierState::default()),
            ttl: DEFAULT_OBSERVATION_TTL,
        }
    }
}

impl ComputerUseObservationCarrier {
    #[cfg(test)]
    fn with_ttl(ttl: Duration) -> Self {
        Self {
            state: Mutex::new(CarrierState::default()),
            ttl,
        }
    }

    pub(crate) fn store(&self, scope_key: String, observation: ComputerUseObservation) {
        let call_id = observation.logical_call_id().to_string();
        let mut state = self.state.lock();
        purge_expired(&mut state, Instant::now());

        if let Some(previous_call_id) = state.call_id_by_scope.remove(&scope_key) {
            state.by_call_id.remove(&previous_call_id);
        }
        if let Some(previous) = state.by_call_id.remove(&call_id) {
            state.call_id_by_scope.remove(&previous.scope_key);
        }

        state
            .call_id_by_scope
            .insert(scope_key.clone(), call_id.clone());
        state.by_call_id.insert(
            call_id,
            PendingObservation {
                scope_key,
                expires_at: Instant::now() + self.ttl,
                observation,
            },
        );
    }

    /// Drain a specific logical call exactly once.
    pub fn take(&self, logical_call_id: &str) -> Option<ComputerUseObservation> {
        let mut state = self.state.lock();
        purge_expired(&mut state, Instant::now());
        let pending = state.by_call_id.remove(logical_call_id)?;
        state.call_id_by_scope.remove(&pending.scope_key);
        Some(pending.observation)
    }

    pub fn invalidate(&self, logical_call_id: &str) -> bool {
        let mut state = self.state.lock();
        let Some(pending) = state.by_call_id.remove(logical_call_id) else {
            return false;
        };
        state.call_id_by_scope.remove(&pending.scope_key);
        true
    }

    pub fn invalidate_all(&self) {
        *self.state.lock() = CarrierState::default();
    }

    pub fn expire(&self) -> usize {
        let mut state = self.state.lock();
        let before = state.by_call_id.len();
        purge_expired(&mut state, Instant::now());
        before - state.by_call_id.len()
    }

    pub fn pending_len(&self) -> usize {
        let mut state = self.state.lock();
        purge_expired(&mut state, Instant::now());
        state.by_call_id.len()
    }
}

fn purge_expired(state: &mut CarrierState, now: Instant) {
    let expired: Vec<String> = state
        .by_call_id
        .iter()
        .filter(|(_, pending)| pending.expires_at <= now)
        .map(|(call_id, _)| call_id.clone())
        .collect();
    for call_id in expired {
        if let Some(pending) = state.by_call_id.remove(&call_id) {
            state.call_id_by_scope.remove(&pending.scope_key);
        }
    }
}

pub(crate) fn capture_observation(
    logical_call_id: &str,
    result: CallToolResult,
) -> Result<ComputerUseObservation, ComputerUseContractError> {
    validate_identity(logical_call_id)?;
    if result.is_error != Some(false) || result.structured_content.is_some() {
        return Err(ComputerUseContractError::UnexpectedContent);
    }

    let mut result_meta = result
        .meta
        .ok_or(ComputerUseContractError::InvalidMetadata)?
        .0;
    if result_meta.len() != 1 {
        return Err(ComputerUseContractError::InvalidMetadata);
    }
    let raw_metadata = result_meta
        .remove(COMPUTER_USE_V2_META_KEY)
        .ok_or(ComputerUseContractError::InvalidMetadata)?;
    let metadata: SnapshotMetadata = serde_json::from_value(raw_metadata)
        .map_err(|_| ComputerUseContractError::InvalidMetadata)?;
    validate_snapshot_metadata(&metadata)?;

    if result.content.len() != 2 {
        return Err(ComputerUseContractError::UnexpectedContent);
    }
    let mut blocks = result.content.into_iter();
    let text = match blocks.next().expect("length checked") {
        ContentBlock::Text(text) if text.meta.is_none() && text.annotations.is_none() => text.text,
        _ => return Err(ComputerUseContractError::UnexpectedContent),
    };
    let image = match blocks.next().expect("length checked") {
        ContentBlock::Image(image)
            if image.mime_type == "image/png"
                && image.meta.is_none()
                && image.annotations.is_none() =>
        {
            image.data
        }
        _ => return Err(ComputerUseContractError::UnexpectedContent),
    };
    if text.len() > MAX_OBSERVATION_BYTES {
        return Err(ComputerUseContractError::ObservationTooLarge);
    }

    let max_encoded_len = MAX_PNG_BYTES.div_ceil(3) * 4;
    if image.len() > max_encoded_len {
        return Err(ComputerUseContractError::ImageTooLarge);
    }
    let png = STANDARD
        .decode(image.as_bytes())
        .map_err(|_| ComputerUseContractError::InvalidImageEncoding)?;
    if png.len() > MAX_PNG_BYTES {
        return Err(ComputerUseContractError::ImageTooLarge);
    }
    if STANDARD.encode(&png) != image {
        return Err(ComputerUseContractError::InvalidImageEncoding);
    }

    let (png_width, png_height) = png_dimensions(&png)?;
    if (png_width, png_height) != (metadata.png_width_px, metadata.png_height_px) {
        return Err(ComputerUseContractError::DimensionMismatch);
    }
    let expected_hash =
        parse_sha256(&metadata.png_sha256).ok_or(ComputerUseContractError::InvalidMetadata)?;
    let actual_hash: [u8; 32] = Sha256::digest(&png).into();
    if actual_hash != expected_hash {
        return Err(ComputerUseContractError::HashMismatch);
    }

    let window_id =
        u32::try_from(metadata.window_id).map_err(|_| ComputerUseContractError::InvalidMetadata)?;
    let geometry = ComputerUseCaptureGeometry {
        origin_x: metadata.capture_origin_x,
        origin_y: metadata.capture_origin_y,
        width_points: metadata.capture_width_points,
        height_points: metadata.capture_height_points,
        png_width,
        png_height,
    };
    Ok(ComputerUseObservation {
        logical_call_id: logical_call_id.to_string().into_boxed_str(),
        snapshot_id: metadata.snapshot_id.into_boxed_str(),
        attestation_id: metadata.attestation_id.into_boxed_str(),
        bundle_id: metadata.bundle_id.into_boxed_str(),
        window_id,
        png_sha256: metadata.png_sha256.to_ascii_lowercase().into_boxed_str(),
        geometry,
        observation: text.into_bytes(),
        png,
    })
}

pub(crate) fn trusted_text_result(
    result: CallToolResult,
) -> Result<String, ComputerUseContractError> {
    if result.structured_content.is_some() || result.meta.is_some() || result.content.len() != 1 {
        return Err(ComputerUseContractError::UnexpectedContent);
    }
    let text = match result.content.into_iter().next().expect("length checked") {
        ContentBlock::Text(text) if text.meta.is_none() && text.annotations.is_none() => text.text,
        _ => return Err(ComputerUseContractError::UnexpectedContent),
    };
    if text.len() > MAX_OBSERVATION_BYTES {
        return Err(ComputerUseContractError::ObservationTooLarge);
    }
    Ok(text)
}

pub(crate) fn validate_lifecycle_result(
    result: CallToolResult,
    expected_success_text: &str,
) -> Result<(), ComputerUseContractError> {
    if result.is_error != Some(false) {
        return Err(ComputerUseContractError::UnexpectedContent);
    }
    let text = trusted_text_result(result)?;
    if text == expected_success_text {
        Ok(())
    } else {
        Err(ComputerUseContractError::UnexpectedContent)
    }
}

fn validate_snapshot_metadata(metadata: &SnapshotMetadata) -> Result<(), ComputerUseContractError> {
    if metadata.profile != COMPUTER_USE_V2_PROFILE_NAME
        || !valid_bounded_id(
            &metadata.snapshot_id,
            MIN_SNAPSHOT_ID_BYTES,
            MAX_SNAPSHOT_ID_BYTES,
        )
        || !valid_bounded_id(
            &metadata.attestation_id,
            MIN_SNAPSHOT_ID_BYTES,
            MAX_SNAPSHOT_ID_BYTES,
        )
        || !valid_bounded_id(&metadata.bundle_id, 3, 255)
        || !(1..=u64::from(u32::MAX)).contains(&metadata.window_id)
        || parse_sha256(&metadata.png_sha256).is_none()
        || metadata.png_width_px == 0
        || metadata.png_height_px == 0
        || metadata.png_width_px > MAX_PNG_SIDE
        || metadata.png_height_px > MAX_PNG_SIDE
        || u64::from(metadata.png_width_px) * u64::from(metadata.png_height_px) > MAX_PNG_PIXELS
        || !metadata.capture_origin_x.is_finite()
        || !metadata.capture_origin_y.is_finite()
        || !metadata.capture_width_points.is_finite()
        || !metadata.capture_height_points.is_finite()
        || metadata.capture_width_points <= 0.0
        || metadata.capture_height_points <= 0.0
    {
        return Err(ComputerUseContractError::InvalidMetadata);
    }
    Ok(())
}

fn valid_bounded_id(value: &str, min: usize, max: usize) -> bool {
    (min..=max).contains(&value.len()) && !value.chars().any(char::is_control)
}

fn png_dimensions(png: &[u8]) -> Result<(u32, u32), ComputerUseContractError> {
    if png.len() < 33
        || &png[..8] != PNG_SIGNATURE
        || u32::from_be_bytes(png[8..12].try_into().expect("fixed slice")) != 13
        || &png[12..16] != b"IHDR"
        // Match the native producer and the sampler's final request-body gate:
        // 8-bit RGBA, deflate compression, standard filter, no interlace.
        || png[24..29] != [8, 6, 0, 0, 0]
    {
        return Err(ComputerUseContractError::InvalidPng);
    }
    let width = u32::from_be_bytes(png[16..20].try_into().expect("fixed slice"));
    let height = u32::from_be_bytes(png[20..24].try_into().expect("fixed slice"));
    if width == 0
        || height == 0
        || width > MAX_PNG_SIDE
        || height > MAX_PNG_SIDE
        || u64::from(width) * u64::from(height) > MAX_PNG_PIXELS
    {
        return Err(ComputerUseContractError::InvalidDimensions);
    }

    let mut offset = 8_usize;
    let mut saw_idat = false;
    let mut idat_ended = false;
    let mut compressed_pixels = Vec::new();
    loop {
        let header_end = offset
            .checked_add(8)
            .filter(|end| *end <= png.len())
            .ok_or(ComputerUseContractError::InvalidPng)?;
        let length = usize::try_from(u32::from_be_bytes(
            png[offset..offset + 4].try_into().expect("fixed slice"),
        ))
        .map_err(|_| ComputerUseContractError::InvalidPng)?;
        let chunk_type = &png[offset + 4..header_end];
        let chunk_end = header_end
            .checked_add(length)
            .and_then(|end| end.checked_add(4))
            .filter(|end| *end <= png.len())
            .ok_or(ComputerUseContractError::InvalidPng)?;
        let data_end = header_end + length;
        let expected_crc =
            u32::from_be_bytes(png[data_end..chunk_end].try_into().expect("fixed slice"));
        let mut crc = crc32fast::Hasher::new();
        crc.update(chunk_type);
        crc.update(&png[header_end..data_end]);
        if crc.finalize() != expected_crc {
            return Err(ComputerUseContractError::InvalidPng);
        }
        if chunk_type == b"IHDR" {
            if offset != 8 {
                return Err(ComputerUseContractError::InvalidPng);
            }
        } else if chunk_type == b"IDAT" {
            if idat_ended {
                return Err(ComputerUseContractError::InvalidPng);
            }
            saw_idat = true;
            compressed_pixels.extend_from_slice(&png[header_end..data_end]);
        } else if chunk_type == b"IEND" {
            if length != 0 || !saw_idat || chunk_end != png.len() {
                return Err(ComputerUseContractError::InvalidPng);
            }
            break;
        } else {
            idat_ended |= saw_idat;
            // Unknown critical chunks are not safe to ignore. Ancillary chunks
            // emitted by ImageIO remain allowed around the contiguous IDAT run.
            if chunk_type[0].is_ascii_uppercase() {
                return Err(ComputerUseContractError::InvalidPng);
            }
        }
        offset = chunk_end;
    }

    let row_bytes = usize::try_from(width)
        .ok()
        .and_then(|width| width.checked_mul(4))
        .and_then(|bytes| bytes.checked_add(1))
        .ok_or(ComputerUseContractError::InvalidPng)?;
    let decoded_len = row_bytes
        .checked_mul(usize::try_from(height).map_err(|_| ComputerUseContractError::InvalidPng)?)
        .ok_or(ComputerUseContractError::InvalidPng)?;
    let decoder = flate2::read::ZlibDecoder::new(compressed_pixels.as_slice());
    let mut bounded = decoder.take(
        u64::try_from(decoded_len)
            .map_err(|_| ComputerUseContractError::InvalidPng)?
            .saturating_add(1),
    );
    let mut decoded = Vec::with_capacity(decoded_len);
    bounded
        .read_to_end(&mut decoded)
        .map_err(|_| ComputerUseContractError::InvalidPng)?;
    let decoder = bounded.into_inner();
    if decoded.len() != decoded_len
        || usize::try_from(decoder.total_in()).ok() != Some(compressed_pixels.len())
        || decoded.chunks_exact(row_bytes).any(|row| row[0] > 4)
    {
        return Err(ComputerUseContractError::InvalidPng);
    }
    Ok((width, height))
}

pub(crate) fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64 {
        return None;
    }
    let mut output = [0_u8; 32];
    for (index, pair) in value.as_bytes().chunks_exact(2).enumerate() {
        output[index] = (hex_nibble(pair[0])? << 4) | hex_nibble(pair[1])?;
    }
    Some(output)
}

#[cfg(test)]
pub(crate) fn test_observation(logical_call_id: &str) -> ComputerUseObservation {
    ComputerUseObservation {
        logical_call_id: logical_call_id.to_string().into_boxed_str(),
        snapshot_id: "test-snapshot-identifier".into(),
        attestation_id: "test-attestation-identifier".into(),
        bundle_id: "com.xai.fixture".into(),
        window_id: 1,
        png_sha256: "a".repeat(64).into_boxed_str(),
        geometry: ComputerUseCaptureGeometry {
            origin_x: 0.0,
            origin_y: 0.0,
            width_points: 1.0,
            height_points: 1.0,
            png_width: 1,
            png_height: 1,
        },
        observation: b"fixture".to_vec(),
        png: Vec::new(),
    }
}

fn hex_nibble(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_container(
        width: u32,
        height: u32,
        bit_depth: u8,
        color_type: u8,
        interlace: u8,
    ) -> Vec<u8> {
        let mut png = Vec::from(PNG_SIGNATURE.as_slice());
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[bit_depth, color_type, 0, 0, interlace]);
        append_chunk(&mut png, b"IHDR", &ihdr);
        let row_bytes = usize::try_from(width).unwrap() * 4 + 1;
        let pixels = vec![0_u8; row_bytes * usize::try_from(height).unwrap()];
        let mut encoder =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut encoder, &pixels).unwrap();
        append_chunk(&mut png, b"IDAT", &encoder.finish().unwrap());
        append_chunk(&mut png, b"IEND", &[]);
        png
    }

    fn append_chunk(png: &mut Vec<u8>, chunk_type: &[u8; 4], data: &[u8]) {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(chunk_type);
        png.extend_from_slice(data);
        let mut crc = crc32fast::Hasher::new();
        crc.update(chunk_type);
        crc.update(data);
        png.extend_from_slice(&crc.finalize().to_be_bytes());
    }

    fn result_with_meta(is_error: bool, protected_meta: bool) -> CallToolResult {
        let mut result = CallToolResult::success(vec![ContentBlock::text("result")]);
        result.is_error = Some(is_error);
        if protected_meta {
            result.meta = Some(Meta(Map::from_iter([(
                COMPUTER_USE_V2_META_KEY.to_string(),
                json!({}),
            )])));
        }
        result
    }

    #[test]
    fn canonical_png_contract_is_exact() {
        assert_eq!(
            png_dimensions(&png_container(17, 23, 8, 6, 0)),
            Ok((17, 23))
        );
        for png in [
            png_container(17, 23, 8, 4, 0),
            png_container(17, 23, 8, 6, 1),
        ] {
            assert_eq!(
                png_dimensions(&png),
                Err(ComputerUseContractError::InvalidPng)
            );
        }

        let mut empty_idat = Vec::from(PNG_SIGNATURE.as_slice());
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&17_u32.to_be_bytes());
        ihdr.extend_from_slice(&23_u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        append_chunk(&mut empty_idat, b"IHDR", &ihdr);
        append_chunk(&mut empty_idat, b"IDAT", &[]);
        append_chunk(&mut empty_idat, b"IEND", &[]);
        assert_eq!(
            png_dimensions(&empty_idat),
            Err(ComputerUseContractError::InvalidPng)
        );
    }

    #[test]
    fn every_successful_snapshot_carrier_is_captured() {
        let plain_success = result_with_meta(false, false);
        let protected_success = result_with_meta(false, true);
        let protected_error = result_with_meta(true, true);
        let mut unspecified = result_with_meta(false, true);
        unspecified.is_error = None;

        assert!(should_capture_observation(
            "get_app_state",
            ComputerUseToolClass::Observation,
            &plain_success,
        ));
        assert!(should_capture_observation(
            "plan_click",
            ComputerUseToolClass::Observation,
            &protected_success,
        ));
        assert!(should_capture_observation(
            "click",
            ComputerUseToolClass::Effectful,
            &protected_success,
        ));
        assert!(!should_capture_observation(
            "click",
            ComputerUseToolClass::Effectful,
            &plain_success,
        ));
        assert!(!should_capture_observation(
            "click",
            ComputerUseToolClass::Effectful,
            &protected_error,
        ));
        assert!(!should_capture_observation(
            "list_apps",
            ComputerUseToolClass::Observation,
            &protected_success,
        ));
        assert!(!should_capture_observation(
            "click",
            ComputerUseToolClass::Effectful,
            &unspecified,
        ));
    }

    #[test]
    fn observation_carrier_expires_entries_at_its_ttl() {
        let carrier = ComputerUseObservationCarrier::with_ttl(Duration::ZERO);
        carrier.store("test-scope".to_string(), test_observation("test-call-id"));

        assert_eq!(carrier.expire(), 1);
        assert_eq!(carrier.pending_len(), 0);
        assert!(carrier.take("test-call-id").is_none());
    }

    #[test]
    fn lifecycle_requests_have_fixed_names_arguments_and_success_text() {
        let attestation = ComputerUseDeliveryAttestation {
            snapshot_id: "snapshot-identifier".into(),
            attestation_id: "attestation-identifier".into(),
            png_sha256: "a".repeat(64).into_boxed_str(),
        };
        let request = ComputerUseLifecycleRequest::AttestSnapshotDelivery(attestation);
        assert_eq!(request.tool_name(), ATTEST_SNAPSHOT_DELIVERY_TOOL);
        assert_eq!(
            request.expected_success_text(),
            "Snapshot delivery attested."
        );
        assert_eq!(
            request.into_arguments(),
            Map::from_iter([
                ("snapshot_id".to_string(), json!("snapshot-identifier")),
                (
                    "attestation_id".to_string(),
                    json!("attestation-identifier"),
                ),
                ("png_sha256".to_string(), json!("a".repeat(64))),
            ]),
        );

        let heartbeat = ComputerUseLifecycleRequest::LeaseHeartbeat {
            snapshot_id: "snapshot-identifier".to_string(),
        };
        assert_eq!(heartbeat.tool_name(), LEASE_HEARTBEAT_TOOL);
        assert_eq!(heartbeat.expected_success_text(), "Desktop lease renewed.");
        assert_eq!(
            heartbeat.into_arguments(),
            Map::from_iter([("snapshot_id".to_string(), json!("snapshot-identifier"),)]),
        );

        assert_eq!(
            validate_lifecycle_result(
                CallToolResult::success(vec![ContentBlock::text("Desktop lease renewed.")]),
                "Desktop lease renewed.",
            ),
            Ok(()),
        );
        assert_eq!(
            validate_lifecycle_result(
                CallToolResult::success(vec![ContentBlock::text("different success")]),
                "Desktop lease renewed.",
            ),
            Err(ComputerUseContractError::UnexpectedContent),
        );
        let mut missing_status =
            CallToolResult::success(vec![ContentBlock::text("Desktop lease renewed.")]);
        missing_status.is_error = None;
        assert_eq!(
            validate_lifecycle_result(missing_status, "Desktop lease renewed."),
            Err(ComputerUseContractError::UnexpectedContent),
        );
    }
}
