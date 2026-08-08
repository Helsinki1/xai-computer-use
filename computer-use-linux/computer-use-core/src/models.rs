//! Shared model types, mirroring `ComputerUseCore/Models.swift`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use time::OffsetDateTime;

pub type JsonObject = Map<String, Value>;

/// The v2 computer-use error taxonomy. Codes and messages match the macOS
/// implementation exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputerUseError {
    InvalidArguments(String),
    InvalidSnapshot,
    SnapshotConsumed,
    DesktopBusy { retry_after_milliseconds: u64 },
    PermissionDenied(String),
    StateUnavailable(String),
    ActionOutcomeUnknown(String),
    InternalFailure(String),
}

impl ComputerUseError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::InvalidSnapshot => "invalid_snapshot",
            Self::SnapshotConsumed => "snapshot_consumed",
            Self::DesktopBusy { .. } => "desktop_busy",
            Self::PermissionDenied(_) => "permission_denied",
            Self::StateUnavailable(_) => "state_unavailable",
            Self::ActionOutcomeUnknown(_) => "outcome_unknown",
            Self::InternalFailure(_) => "internal_failure",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::InvalidArguments(message)
            | Self::PermissionDenied(message)
            | Self::StateUnavailable(message)
            | Self::ActionOutcomeUnknown(message)
            | Self::InternalFailure(message) => message.clone(),
            Self::InvalidSnapshot => {
                "The snapshot is unknown, expired, or belongs to another client. Call get_app_state.".to_owned()
            }
            Self::SnapshotConsumed => {
                "The snapshot has already authorized an action. Call get_app_state before another action.".to_owned()
            }
            Self::DesktopBusy { retry_after_milliseconds } => format!(
                "Another client holds the desktop lease. Retry after {retry_after_milliseconds} ms."
            ),
        }
    }
}

impl std::fmt::Display for ComputerUseError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message())
    }
}

impl std::error::Error for ComputerUseError {}

pub type Result<T> = std::result::Result<T, ComputerUseError>;

/// A running GUI application. On Linux, `bundle_identifier` carries the
/// window-manager class name (WM_CLASS class), the closest stable analogue of
/// a macOS bundle identifier on X11.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppDescriptor {
    pub name: String,
    #[serde(rename = "bundleIdentifier", skip_serializing_if = "Option::is_none")]
    pub bundle_identifier: Option<String>,
    #[serde(rename = "processIdentifier")]
    pub process_identifier: i32,
    #[serde(rename = "isActive")]
    pub is_active: bool,
    #[serde(rename = "windowIdentifiers", default)]
    pub window_identifiers: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppTarget {
    pub name: String,
    #[serde(rename = "bundleIdentifier", skip_serializing_if = "Option::is_none")]
    pub bundle_identifier: Option<String>,
    #[serde(rename = "processIdentifier")]
    pub process_identifier: i32,
}

/// A continuous coordinate in the snapshot PNG's edge space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PngPixelPoint {
    pub x: f64,
    pub y: f64,
}

/// A point in the global desktop coordinate space (X11 root coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GlobalScreenPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GlobalScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl GlobalScreenRect {
    pub fn center(&self) -> GlobalScreenPoint {
        GlobalScreenPoint {
            x: self.x + self.width / 2.0,
            y: self.y + self.height / 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    #[serde(rename = "windowIdentifier")]
    pub window_identifier: u32,
    #[serde(rename = "globalBoundsPoints")]
    pub global_bounds_points: GlobalScreenRect,
    #[serde(rename = "pngWidthPixels")]
    pub png_width_pixels: u32,
    #[serde(rename = "pngHeightPixels")]
    pub png_height_pixels: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccessibilityElementSnapshot {
    pub identifier: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<GlobalScreenRect>,
    pub actions: Vec<String>,
    #[serde(rename = "isValueSettable")]
    pub is_value_settable: bool,
    #[serde(rename = "driverToken")]
    pub driver_token: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CapturedDesktopState {
    pub app: AppTarget,
    pub window_title: Option<String>,
    pub geometry: WindowGeometry,
    pub screenshot_png: Vec<u8>,
    pub screenshot_sha256: String,
    pub accessibility_tree: String,
    pub elements: Vec<AccessibilityElementSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotEnvelope {
    pub snapshot_identifier: String,
    pub delivery_attestation_identifier: String,
    pub captured: CapturedDesktopState,
    pub lease_expires_at: OffsetDateTime,
}

/// The protected observation carrier. Field names and validation bounds match
/// the macOS `computer-use-v2` profile exactly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtectedComputerUseCarrier {
    pub profile: String,
    #[serde(rename = "snapshotID")]
    pub snapshot_id: String,
    #[serde(rename = "attestationID")]
    pub attestation_id: String,
    #[serde(rename = "bundleID")]
    pub bundle_id: String,
    #[serde(rename = "windowID")]
    pub window_id: u32,
    #[serde(rename = "pngSHA256")]
    pub png_sha256: String,
    #[serde(rename = "pngWidth")]
    pub png_width: u32,
    #[serde(rename = "pngHeight")]
    pub png_height: u32,
    #[serde(rename = "captureOriginX")]
    pub capture_origin_x: f64,
    #[serde(rename = "captureOriginY")]
    pub capture_origin_y: f64,
    #[serde(rename = "captureWidthPoints")]
    pub capture_width_points: f64,
    #[serde(rename = "captureHeightPoints")]
    pub capture_height_points: f64,
}

pub const MAX_PROTECTED_PNG_BYTES: usize = 900_000;
pub const MAX_PNG_SIDE_PIXELS: u32 = 1_280;
pub const MAX_PNG_TOTAL_PIXELS: u64 = 1_638_400;

impl ProtectedComputerUseCarrier {
    pub fn new(snapshot: &SnapshotEnvelope) -> Result<Self> {
        let captured = &snapshot.captured;
        let geometry = &captured.geometry;
        let bounds = &geometry.global_bounds_points;
        let bundle_id = captured
            .app
            .bundle_identifier
            .clone()
            .filter(|identifier| (3..=255).contains(&identifier.len()));
        let incomplete = || {
            ComputerUseError::StateUnavailable(
                "The protected snapshot metadata is incomplete.".to_owned(),
            )
        };
        let bundle_id = bundle_id.ok_or_else(incomplete)?;
        let valid = (16..=128).contains(&snapshot.snapshot_identifier.len())
            && (16..=128).contains(&snapshot.delivery_attestation_identifier.len())
            && captured.screenshot_sha256.len() == 64
            && captured
                .screenshot_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && !captured.screenshot_png.is_empty()
            && captured.screenshot_png.len() <= MAX_PROTECTED_PNG_BYTES
            && (1..=MAX_PNG_SIDE_PIXELS).contains(&geometry.png_width_pixels)
            && (1..=MAX_PNG_SIDE_PIXELS).contains(&geometry.png_height_pixels)
            && u64::from(geometry.png_width_pixels) * u64::from(geometry.png_height_pixels)
                <= MAX_PNG_TOTAL_PIXELS
            && geometry.window_identifier > 0
            && bounds.x.is_finite()
            && bounds.y.is_finite()
            && bounds.width.is_finite()
            && bounds.height.is_finite()
            && bounds.width > 0.0
            && bounds.height > 0.0;
        if !valid {
            return Err(incomplete());
        }
        Ok(Self {
            profile: "computer-use-v2".to_owned(),
            snapshot_id: snapshot.snapshot_identifier.clone(),
            attestation_id: snapshot.delivery_attestation_identifier.clone(),
            bundle_id,
            window_id: geometry.window_identifier,
            png_sha256: captured.screenshot_sha256.clone(),
            png_width: geometry.png_width_pixels,
            png_height: geometry.png_height_pixels,
            capture_origin_x: bounds.x,
            capture_origin_y: bounds.y,
            capture_width_points: bounds.width,
            capture_height_points: bounds.height,
        })
    }

    /// The snake_case observation JSON consumed by the Rust MCP host.
    pub fn observation_json(&self) -> Value {
        serde_json::json!({
            "profile": self.profile,
            "snapshot_id": self.snapshot_id,
            "attestation_id": self.attestation_id,
            "bundle_id": self.bundle_id,
            "window_id": self.window_id,
            "png_sha256": self.png_sha256,
            "png_width_px": self.png_width,
            "png_height_px": self.png_height,
            "capture_origin_x": self.capture_origin_x,
            "capture_origin_y": self.capture_origin_y,
            "capture_width_points": self.capture_width_points,
            "capture_height_points": self.capture_height_points,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolExecutionResult {
    pub text: String,
    #[serde(rename = "structuredContent", skip_serializing_if = "Option::is_none")]
    pub structured_content: Option<Value>,
    #[serde(
        rename = "imagePNG",
        skip_serializing_if = "Option::is_none",
        with = "optional_base64",
        default
    )]
    pub image_png: Option<Vec<u8>>,
    #[serde(rename = "isError")]
    pub is_error: bool,
    #[serde(rename = "protectedCarrier", skip_serializing_if = "Option::is_none")]
    pub protected_carrier: Option<ProtectedComputerUseCarrier>,
}

impl ToolExecutionResult {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured_content: None,
            image_png: None,
            is_error: false,
            protected_carrier: None,
        }
    }

    pub fn error(error: &ComputerUseError) -> Self {
        Self {
            text: format!("{}: {}", error.code(), error.message()),
            structured_content: None,
            image_png: None,
            is_error: true,
            protected_carrier: None,
        }
    }
}

mod optional_base64 {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Vec<u8>>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => {
                serializer.serialize_str(&base64::engine::general_purpose::STANDARD.encode(bytes))
            }
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Option<Vec<u8>>, D::Error> {
        let encoded = Option::<String>::deserialize(deserializer)?;
        encoded
            .map(|encoded| {
                base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "middle" => Some(Self::Middle),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallContext {
    pub client_identifier: String,
    pub action_identifier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ActionReceiptState {
    #[serde(rename = "prepared")]
    Prepared,
    #[serde(rename = "dispatched")]
    Dispatched,
    #[serde(rename = "applied")]
    Applied,
    #[serde(rename = "rejected")]
    Rejected,
    #[serde(rename = "outcome_unknown")]
    OutcomeUnknown,
}

impl ActionReceiptState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Dispatched => "dispatched",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "prepared" => Some(Self::Prepared),
            "dispatched" => Some(Self::Dispatched),
            "applied" => Some(Self::Applied),
            "rejected" => Some(Self::Rejected),
            "outcome_unknown" => Some(Self::OutcomeUnknown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionReceipt {
    pub identifier: String,
    #[serde(rename = "toolName")]
    pub tool_name: String,
    #[serde(rename = "snapshotIdentifier")]
    pub snapshot_identifier: String,
    pub state: ActionReceiptState,
    #[serde(rename = "createdAt", with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(rename = "updatedAt", with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    #[serde(rename = "failureCode", skip_serializing_if = "Option::is_none")]
    pub failure_code: Option<String>,
}

impl ActionReceipt {
    pub fn transitioned(
        &self,
        state: ActionReceiptState,
        at: OffsetDateTime,
        failure_code: Option<String>,
    ) -> Self {
        Self {
            identifier: self.identifier.clone(),
            tool_name: self.tool_name.clone(),
            snapshot_identifier: self.snapshot_identifier.clone(),
            state,
            created_at: self.created_at,
            updated_at: at,
            failure_code,
        }
    }
}

/// Rejects non-finite numbers anywhere in a JSON value. `serde_json` cannot
/// parse non-finite literals from text, but values constructed in process can
/// carry them; validation keeps the wire contract airtight.
pub fn validate_finite_numbers(value: &Value) -> Result<()> {
    match value {
        Value::Number(number) => {
            if number.as_f64().is_some_and(f64::is_finite)
                || number.as_i64().is_some()
                || number.as_u64().is_some()
            {
                Ok(())
            } else {
                Err(ComputerUseError::InvalidArguments(
                    "JSON numbers must be finite".to_owned(),
                ))
            }
        }
        Value::Array(items) => items.iter().try_for_each(validate_finite_numbers),
        Value::Object(entries) => entries.values().try_for_each(validate_finite_numbers),
        _ => Ok(()),
    }
}

/// Truncates a string to at most `maximum_bytes` UTF-8 bytes on a character
/// boundary, mirroring the macOS `boundedUTF8` helper.
pub fn bounded_utf8(value: &str, maximum_bytes: usize) -> String {
    if value.len() <= maximum_bytes {
        return value.to_owned();
    }
    let mut end = maximum_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].to_owned()
}
