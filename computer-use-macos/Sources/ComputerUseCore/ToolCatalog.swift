import Foundation

public struct ToolDefinition: Sendable, Equatable {
    public let name: String
    public let description: String
    public let inputSchema: JSONValue
    public let annotations: JSONValue

    public init(name: String, description: String, inputSchema: JSONValue, annotations: JSONValue) {
        self.name = name
        self.description = description
        self.inputSchema = inputSchema
        self.annotations = annotations
    }

    public var json: JSONValue {
        .object([
            "name": .string(name),
            "description": .string(description),
            "inputSchema": inputSchema,
            "annotations": annotations,
        ])
    }
}

public enum ToolCatalog {
    public static let hiddenToolNames: Set<String> = [
        "attest_snapshot_delivery",
        "invalidate_session",
        "lease_heartbeat",
        "release_operation",
    ]

    public static var acceptedToolNames: Set<String> {
        Set(all.map(\.name)).union(hiddenToolNames)
    }

    public static let actionToolNames: Set<String> = [
        "click",
        "perform_secondary_action",
        "scroll",
        "drag",
        "type_text",
        "press_key",
        "set_value",
    ]

    public static let all: [ToolDefinition] = [
        ToolDefinition(
            name: "list_apps",
            description: "List running GUI applications that can be selected for computer use. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(properties: [:], required: []),
            annotations: readOnlyAnnotations
        ),
        ToolDefinition(
            name: "get_app_state",
            description: "Acquire the serialized desktop lease and return a fresh, server-authoritative screenshot, accessibility snapshot, and bounded target_candidates map. Call this before every action sequence, as the only computer-use tool call in this model turn. Match the user's visual intent against the current screenshot and target_candidates; never reuse a target or coordinate from an earlier snapshot. System Settings is unavailable to computer use.",
            inputSchema: objectSchema(
                properties: [
                    "bundle_id": boundedStringSchema(minimum: 3, maximum: 255),
                    "window_id": integerRangeSchema(minimum: 1, maximum: 4_294_967_295),
                ],
                required: ["bundle_id"]
            ),
            annotations: readOnlyAnnotations
        ),
        ToolDefinition(
            name: "plan_click",
            description: "Resolve and describe the exact click that would be sent for this fresh, attested snapshot without dispatching input or consuming the snapshot. Use this when a target is uncertain, then call click with the same snapshot_id and target only if the plan matches the intended control.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "target": .object([
                        "oneOf": .array([elementTargetSchema, pixelTargetSchema]),
                    ]),
                    "button": enumSchema(["left", "right"]),
                    "count": integerEnumSchema([1, 2]),
                ],
                required: ["snapshot_id", "target"]
            ),
            annotations: readOnlyAnnotations
        ),
        ToolDefinition(
            name: "click",
            description: "Consume a snapshot and click either an accessibility element or a point in that snapshot's PNG pixel-edge coordinate space. Prefer target.kind=element when a target_candidate matches the intended control and offers AXPress (for example, intent 'open Settings' + candidate label 'Settings' -> click that element). Use target.kind=pixel only when no matching actionable element exists, and only with coordinates from this exact screenshot. For a small, dense, or ambiguous target where you are not confident of the exact point or element, call plan_click first to resolve and confirm it before dispatching. Never use a control intended to open System Settings. If the expected effect is absent, acquire fresh state and re-identify the target; do not replay an uncertain action. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "target": .object([
                        "oneOf": .array([elementTargetSchema, pixelTargetSchema]),
                    ]),
                    "button": enumSchema(["left", "right"]),
                    "count": integerEnumSchema([1, 2]),
                ],
                required: ["snapshot_id", "target"]
            ),
            annotations: actionAnnotations
        ),
        ToolDefinition(
            name: "perform_secondary_action",
            description: "Consume a snapshot and invoke a non-primary accessibility action advertised by an element. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "element_id": elementIDSchema,
                    "action_id": boundedStringSchema(minimum: 1, maximum: 128),
                ],
                required: ["snapshot_id", "element_id", "action_id"]
            ),
            annotations: actionAnnotations
        ),
        ToolDefinition(
            name: "scroll",
            description: "Consume a snapshot and scroll at an accessibility element in a cardinal direction. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "element_id": elementIDSchema,
                    "direction": enumSchema(["up", "down", "left", "right"]),
                    "pages": .object([
                        "type": .string("number"),
                        "exclusiveMinimum": .integer(0),
                        "maximum": .integer(10),
                    ]),
                ],
                required: ["snapshot_id", "element_id", "direction"]
            ),
            annotations: actionAnnotations
        ),
        ToolDefinition(
            name: "drag",
            description: "Consume a snapshot and drag between two points in its PNG pixel-edge coordinate space. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "from": pixelPointSchema,
                    "to": pixelPointSchema,
                ],
                required: ["snapshot_id", "from", "to"]
            ),
            annotations: actionAnnotations
        ),
        ToolDefinition(
            name: "type_text",
            description: "Consume a snapshot and type literal text into its target application. Text is never logged or written to receipts. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "text": .object(["type": .string("string"), "maxLength": .integer(32_768)]),
                ],
                required: ["snapshot_id", "text"]
            ),
            annotations: actionAnnotations
        ),
        ToolDefinition(
            name: "press_key",
            description: "Consume a snapshot and press one named key, with modifiers passed separately. Navigation names include left, right, up, down, home, end, pageup, and pagedown; common aliases such as ArrowRight and RightArrow are accepted. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "key": .object(["type": .string("string")]),
                    "modifiers": .object([
                        "type": .string("array"),
                        "items": .object([
                            "type": .string("string"),
                            "enum": .array(["command", "control", "option", "shift", "fn"].map(JSONValue.string)),
                        ]),
                        "uniqueItems": .bool(true),
                    ]),
                ],
                required: ["snapshot_id", "key"]
            ),
            annotations: actionAnnotations
        ),
        ToolDefinition(
            name: "set_value",
            description: "Consume a snapshot and assign the value of an accessibility element that was marked settable. Issue exactly one computer-use tool call in this model turn.",
            inputSchema: objectSchema(
                properties: [
                    "snapshot_id": snapshotIDSchema,
                    "element_id": elementIDSchema,
                    "value": .object(["type": .string("string"), "maxLength": .integer(32_768)]),
                ],
                required: ["snapshot_id", "element_id", "value"]
            ),
            annotations: actionAnnotations
        ),
    ]

    private static let actionAnnotations: JSONValue = .object([
        "destructiveHint": .bool(false),
        "openWorldHint": .bool(false),
        "readOnlyHint": .bool(false),
    ])

    private static let readOnlyAnnotations: JSONValue = .object([
        "destructiveHint": .bool(false),
        "idempotentHint": .bool(true),
        "openWorldHint": .bool(false),
        "readOnlyHint": .bool(true),
    ])

    private static func objectSchema(
        properties: [String: JSONValue],
        required: [String],
        oneOfRequired: [[String]] = []
    ) -> JSONValue {
        var schema: [String: JSONValue] = [
            "type": .string("object"),
            "properties": .object(properties),
            "additionalProperties": .bool(false),
        ]
        if !required.isEmpty {
            schema["required"] = .array(required.map(JSONValue.string))
        }
        if !oneOfRequired.isEmpty {
            schema["oneOf"] = .array(oneOfRequired.map { keys in
                .object(["required": .array(keys.map(JSONValue.string))])
            })
        }
        return .object(schema)
    }

    private static let snapshotIDSchema: JSONValue = boundedStringSchema(minimum: 16, maximum: 128)
    private static let elementIDSchema: JSONValue = boundedStringSchema(minimum: 1, maximum: 128)

    private static let pixelPointSchema: JSONValue = objectSchema(
        properties: [
            "x_px": .object(["type": .string("number")]),
            "y_px": .object(["type": .string("number")]),
        ],
        required: ["x_px", "y_px"]
    )

    private static let elementTargetSchema: JSONValue = objectSchema(
        properties: [
            "kind": .object(["const": .string("element")]),
            "element_id": elementIDSchema,
        ],
        required: ["kind", "element_id"]
    )

    private static let pixelTargetSchema: JSONValue = objectSchema(
        properties: [
            "kind": .object(["const": .string("pixel")]),
            "x_px": .object(["type": .string("number")]),
            "y_px": .object(["type": .string("number")]),
        ],
        required: ["kind", "x_px", "y_px"]
    )

    private static func boundedStringSchema(minimum: Int, maximum: Int) -> JSONValue {
        .object([
            "type": .string("string"),
            "minLength": .integer(Int64(minimum)),
            "maxLength": .integer(Int64(maximum)),
        ])
    }

    private static func integerRangeSchema(minimum: Int64, maximum: Int64) -> JSONValue {
        .object([
            "type": .string("integer"),
            "minimum": .integer(minimum),
            "maximum": .integer(maximum),
        ])
    }

    private static func integerEnumSchema(_ values: [Int]) -> JSONValue {
        .object([
            "type": .string("integer"),
            "enum": .array(values.map { .integer(Int64($0)) }),
        ])
    }

    private static func enumSchema(_ values: [String]) -> JSONValue {
        .object([
            "type": .string("string"),
            "enum": .array(values.map(JSONValue.string)),
        ])
    }
}

struct ArgumentReader {
    private var remaining: [String: JSONValue]

    init(_ arguments: [String: JSONValue]) {
        remaining = arguments
    }

    mutating func requiredString(_ key: String, allowEmpty: Bool = false) throws -> String {
        guard let value = remaining.removeValue(forKey: key)?.stringValue,
              allowEmpty || !value.isEmpty
        else {
            throw ComputerUseError.invalidArguments("\(key) must be a string\(allowEmpty ? "" : " with at least one character").")
        }
        return value
    }

    mutating func optionalString(_ key: String) throws -> String? {
        guard let raw = remaining.removeValue(forKey: key) else { return nil }
        guard let value = raw.stringValue, !value.isEmpty else {
            throw ComputerUseError.invalidArguments("\(key) must be a non-empty string.")
        }
        return value
    }

    mutating func requiredNumber(_ key: String) throws -> Double {
        guard let value = remaining.removeValue(forKey: key)?.doubleValue, value.isFinite else {
            throw ComputerUseError.invalidArguments("\(key) must be a finite number.")
        }
        return value
    }

    mutating func optionalNumber(_ key: String, default defaultValue: Double) throws -> Double {
        guard let raw = remaining.removeValue(forKey: key) else { return defaultValue }
        guard let value = raw.doubleValue, value.isFinite else {
            throw ComputerUseError.invalidArguments("\(key) must be a finite number.")
        }
        return value
    }

    mutating func optionalInteger(_ key: String, default defaultValue: Int) throws -> Int {
        guard let raw = remaining.removeValue(forKey: key) else { return defaultValue }
        guard let value = raw.intValue else {
            throw ComputerUseError.invalidArguments("\(key) must be an integer.")
        }
        return value
    }

    mutating func optionalInteger(_ key: String) throws -> Int? {
        guard let raw = remaining.removeValue(forKey: key) else { return nil }
        guard let value = raw.intValue else {
            throw ComputerUseError.invalidArguments("\(key) must be an integer.")
        }
        return value
    }

    mutating func requiredObject(_ key: String) throws -> [String: JSONValue] {
        guard let value = remaining.removeValue(forKey: key)?.objectValue else {
            throw ComputerUseError.invalidArguments("\(key) must be an object.")
        }
        return value
    }

    mutating func optionalStringArray(_ key: String) throws -> [String] {
        guard let raw = remaining.removeValue(forKey: key) else { return [] }
        guard case let .array(values) = raw else {
            throw ComputerUseError.invalidArguments("\(key) must be an array.")
        }
        let strings = values.compactMap(\.stringValue)
        guard strings.count == values.count else {
            throw ComputerUseError.invalidArguments("\(key) must contain only strings.")
        }
        return strings
    }

    mutating func finish() throws {
        guard remaining.isEmpty else {
            throw ComputerUseError.invalidArguments("Unknown argument fields: \(remaining.keys.sorted().joined(separator: ", ")).")
        }
    }
}
