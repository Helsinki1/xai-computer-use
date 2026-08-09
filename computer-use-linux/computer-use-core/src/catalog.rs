//! The exact v2 tool catalog, mirroring `ToolCatalog.swift`.

use serde_json::{json, Value};

pub const HIDDEN_TOOL_NAMES: [&str; 4] = [
    "attest_snapshot_delivery",
    "invalidate_session",
    "lease_heartbeat",
    "release_operation",
];

pub const ACTION_TOOL_NAMES: [&str; 7] = [
    "click",
    "perform_secondary_action",
    "scroll",
    "drag",
    "type_text",
    "press_key",
    "set_value",
];

pub fn is_hidden_tool(name: &str) -> bool {
    HIDDEN_TOOL_NAMES.contains(&name)
}

pub fn is_action_tool(name: &str) -> bool {
    ACTION_TOOL_NAMES.contains(&name)
}

pub fn is_accepted_tool(name: &str) -> bool {
    is_hidden_tool(name) || all().iter().any(|tool| tool.name == name)
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub annotations: Value,
}

impl ToolDefinition {
    pub fn json(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": self.annotations,
        })
    }
}

pub fn all() -> Vec<ToolDefinition> {
    let action_annotations = json!({
        "destructiveHint": false,
        "openWorldHint": false,
        "readOnlyHint": false,
    });
    let read_only_annotations = json!({
        "destructiveHint": false,
        "idempotentHint": true,
        "openWorldHint": false,
        "readOnlyHint": true,
    });
    let snapshot_id = bounded_string(16, 128);
    let element_id = bounded_string(1, 128);
    let pixel_point = object_schema(
        json!({
            "x_px": {"type": "number"},
            "y_px": {"type": "number"},
        }),
        &["x_px", "y_px"],
    );
    let element_target = object_schema(
        json!({
            "kind": {"const": "element"},
            "element_id": element_id,
        }),
        &["kind", "element_id"],
    );
    let pixel_target = object_schema(
        json!({
            "kind": {"const": "pixel"},
            "x_px": {"type": "number"},
            "y_px": {"type": "number"},
        }),
        &["kind", "x_px", "y_px"],
    );

    vec![
        ToolDefinition {
            name: "list_apps",
            description: "List running GUI applications that can be selected for computer use.",
            input_schema: object_schema(json!({}), &[]),
            annotations: read_only_annotations.clone(),
        },
        ToolDefinition {
            name: "get_app_state",
            description: "Acquire the serialized desktop lease and return a fresh, server-authoritative screenshot plus accessibility snapshot. Call this before every action sequence.",
            input_schema: object_schema(
                json!({
                    "bundle_id": bounded_string(3, 255),
                    "window_id": {"type": "integer", "minimum": 1, "maximum": 4_294_967_295_u64},
                }),
                &["bundle_id"],
            ),
            annotations: read_only_annotations,
        },
        ToolDefinition {
            name: "click",
            description: "Consume a snapshot and click either an accessibility element or a point in that snapshot's PNG pixel-edge coordinate space.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "target": {"oneOf": [element_target, pixel_target]},
                    "button": {"type": "string", "enum": ["left", "right"]},
                    "count": {"type": "integer", "enum": [1, 2]},
                }),
                &["snapshot_id", "target"],
            ),
            annotations: action_annotations.clone(),
        },
        ToolDefinition {
            name: "perform_secondary_action",
            description: "Consume a snapshot and invoke a non-primary accessibility action advertised by an element.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "element_id": element_id,
                    "action_id": bounded_string(1, 128),
                }),
                &["snapshot_id", "element_id", "action_id"],
            ),
            annotations: action_annotations.clone(),
        },
        ToolDefinition {
            name: "scroll",
            description: "Consume a snapshot and scroll at an accessibility element in a cardinal direction.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "element_id": element_id,
                    "direction": {"type": "string", "enum": ["up", "down", "left", "right"]},
                    "pages": {"type": "number", "exclusiveMinimum": 0, "maximum": 10},
                }),
                &["snapshot_id", "element_id", "direction"],
            ),
            annotations: action_annotations.clone(),
        },
        ToolDefinition {
            name: "drag",
            description: "Consume a snapshot and drag between two points in its PNG pixel-edge coordinate space.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "from": pixel_point,
                    "to": pixel_point,
                }),
                &["snapshot_id", "from", "to"],
            ),
            annotations: action_annotations.clone(),
        },
        ToolDefinition {
            name: "type_text",
            description: "Consume a snapshot and type literal text into its target application. Text is never logged or written to receipts.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "text": {"type": "string", "maxLength": 32_768},
                }),
                &["snapshot_id", "text"],
            ),
            annotations: action_annotations.clone(),
        },
        ToolDefinition {
            name: "press_key",
            description: "Consume a snapshot and press one named key or modifier combination such as cmd+c, Return, or Shift+Tab.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "key": {"type": "string"},
                    "modifiers": {
                        "type": "array",
                        "items": {
                            "type": "string",
                            "enum": ["command", "control", "option", "shift", "fn"],
                        },
                        "uniqueItems": true,
                    },
                }),
                &["snapshot_id", "key"],
            ),
            annotations: action_annotations.clone(),
        },
        ToolDefinition {
            name: "set_value",
            description: "Consume a snapshot and assign the value of an accessibility element that was marked settable.",
            input_schema: object_schema(
                json!({
                    "snapshot_id": snapshot_id,
                    "element_id": element_id,
                    "value": {"type": "string", "maxLength": 32_768},
                }),
                &["snapshot_id", "element_id", "value"],
            ),
            annotations: action_annotations,
        },
    ]
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
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

fn bounded_string(minimum: u64, maximum: u64) -> Value {
    json!({"type": "string", "minLength": minimum, "maxLength": maximum})
}
