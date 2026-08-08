import Foundation

public enum JSONValue: Sendable, Equatable, Codable {
    case null
    case bool(Bool)
    case integer(Int64)
    case number(Double)
    case string(String)
    case array([JSONValue])
    case object([String: JSONValue])

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .integer(value)
        } else if let value = try? container.decode(Double.self) {
            guard value.isFinite else {
                throw DecodingError.dataCorruptedError(in: container, debugDescription: "JSON numbers must be finite")
            }
            self = .number(value)
        } else if let value = try? container.decode(String.self) {
            self = .string(value)
        } else if let value = try? container.decode([JSONValue].self) {
            self = .array(value)
        } else if let value = try? container.decode([String: JSONValue].self) {
            self = .object(value)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "Unsupported JSON value")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .null:
            try container.encodeNil()
        case let .bool(value):
            try container.encode(value)
        case let .integer(value):
            try container.encode(value)
        case let .number(value):
            guard value.isFinite else {
                throw EncodingError.invalidValue(value, .init(codingPath: encoder.codingPath, debugDescription: "JSON numbers must be finite"))
            }
            try container.encode(value)
        case let .string(value):
            try container.encode(value)
        case let .array(value):
            try container.encode(value)
        case let .object(value):
            try container.encode(value)
        }
    }

    public var objectValue: [String: JSONValue]? {
        guard case let .object(value) = self else { return nil }
        return value
    }

    public var stringValue: String? {
        guard case let .string(value) = self else { return nil }
        return value
    }

    public var boolValue: Bool? {
        guard case let .bool(value) = self else { return nil }
        return value
    }

    public var doubleValue: Double? {
        switch self {
        case let .integer(value): return Double(value)
        case let .number(value): return value
        default: return nil
        }
    }

    public var intValue: Int? {
        switch self {
        case let .integer(value): return Int(exactly: value)
        default: return nil
        }
    }
}

public enum ComputerUseError: Error, Sendable, Equatable {
    case invalidArguments(String)
    case invalidSnapshot
    case snapshotConsumed
    case desktopBusy(retryAfterMilliseconds: Int)
    case permissionDenied(String)
    case stateUnavailable(String)
    case actionOutcomeUnknown(String)
    case internalFailure(String)

    public var code: String {
        switch self {
        case .invalidArguments: "invalid_arguments"
        case .invalidSnapshot: "invalid_snapshot"
        case .snapshotConsumed: "snapshot_consumed"
        case .desktopBusy: "desktop_busy"
        case .permissionDenied: "permission_denied"
        case .stateUnavailable: "state_unavailable"
        case .actionOutcomeUnknown: "outcome_unknown"
        case .internalFailure: "internal_failure"
        }
    }

    public var message: String {
        switch self {
        case let .invalidArguments(message),
             let .permissionDenied(message),
             let .stateUnavailable(message),
             let .actionOutcomeUnknown(message),
             let .internalFailure(message):
            message
        case .invalidSnapshot:
            "The snapshot is unknown, expired, or belongs to another client. Call get_app_state."
        case .snapshotConsumed:
            "The snapshot has already authorized an action. Call get_app_state before another action."
        case let .desktopBusy(retryAfterMilliseconds):
            "Another client holds the desktop lease. Retry after \(retryAfterMilliseconds) ms."
        }
    }
}

extension ComputerUseError: LocalizedError {
    public var errorDescription: String? { message }
}

public struct AppDescriptor: Codable, Sendable, Equatable {
    public let name: String
    public let bundleIdentifier: String?
    public let processIdentifier: Int32
    public let isActive: Bool
    public let windowIdentifiers: [UInt32]

    public init(
        name: String,
        bundleIdentifier: String?,
        processIdentifier: Int32,
        isActive: Bool,
        windowIdentifiers: [UInt32] = []
    ) {
        self.name = name
        self.bundleIdentifier = bundleIdentifier
        self.processIdentifier = processIdentifier
        self.isActive = isActive
        self.windowIdentifiers = windowIdentifiers
    }
}

public struct AppTarget: Codable, Sendable, Equatable, Hashable {
    public let name: String
    public let bundleIdentifier: String?
    public let processIdentifier: Int32

    public init(name: String, bundleIdentifier: String?, processIdentifier: Int32) {
        self.name = name
        self.bundleIdentifier = bundleIdentifier
        self.processIdentifier = processIdentifier
    }
}

public struct PNGPixelPoint: Codable, Sendable, Equatable {
    public let x: Double
    public let y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public struct GlobalScreenPoint: Codable, Sendable, Equatable {
    public let x: Double
    public let y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public struct GlobalScreenRect: Codable, Sendable, Equatable {
    public let x: Double
    public let y: Double
    public let width: Double
    public let height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    public var center: GlobalScreenPoint {
        GlobalScreenPoint(x: x + width / 2, y: y + height / 2)
    }
}

public struct WindowGeometry: Codable, Sendable, Equatable {
    public let windowIdentifier: UInt32
    public let globalBoundsPoints: GlobalScreenRect
    public let pngWidthPixels: Int
    public let pngHeightPixels: Int

    public init(windowIdentifier: UInt32, globalBoundsPoints: GlobalScreenRect, pngWidthPixels: Int, pngHeightPixels: Int) {
        self.windowIdentifier = windowIdentifier
        self.globalBoundsPoints = globalBoundsPoints
        self.pngWidthPixels = pngWidthPixels
        self.pngHeightPixels = pngHeightPixels
    }
}

public struct AccessibilityElementSnapshot: Codable, Sendable, Equatable {
    public let identifier: String
    public let role: String
    public let label: String?
    public let value: String?
    public let frame: GlobalScreenRect?
    public let actions: [String]
    public let isValueSettable: Bool
    public let driverToken: String

    public init(
        identifier: String,
        role: String,
        label: String?,
        value: String?,
        frame: GlobalScreenRect?,
        actions: [String],
        isValueSettable: Bool,
        driverToken: String
    ) {
        self.identifier = identifier
        self.role = role
        self.label = label
        self.value = value
        self.frame = frame
        self.actions = actions
        self.isValueSettable = isValueSettable
        self.driverToken = driverToken
    }
}

public struct CapturedDesktopState: Sendable, Equatable {
    public let app: AppTarget
    public let windowTitle: String?
    public let geometry: WindowGeometry
    public let screenshotPNG: Data
    public let screenshotSHA256: String
    public let accessibilityTree: String
    public let elements: [AccessibilityElementSnapshot]

    public init(
        app: AppTarget,
        windowTitle: String?,
        geometry: WindowGeometry,
        screenshotPNG: Data,
        screenshotSHA256: String,
        accessibilityTree: String,
        elements: [AccessibilityElementSnapshot]
    ) {
        self.app = app
        self.windowTitle = windowTitle
        self.geometry = geometry
        self.screenshotPNG = screenshotPNG
        self.screenshotSHA256 = screenshotSHA256
        self.accessibilityTree = accessibilityTree
        self.elements = elements
    }
}

public struct SnapshotEnvelope: Sendable, Equatable {
    public let snapshotIdentifier: String
    public let deliveryAttestationIdentifier: String
    public let captured: CapturedDesktopState
    public let leaseExpiresAt: Date

    public init(snapshotIdentifier: String, deliveryAttestationIdentifier: String, captured: CapturedDesktopState, leaseExpiresAt: Date) {
        self.snapshotIdentifier = snapshotIdentifier
        self.deliveryAttestationIdentifier = deliveryAttestationIdentifier
        self.captured = captured
        self.leaseExpiresAt = leaseExpiresAt
    }
}

public struct ProtectedComputerUseCarrier: Codable, Sendable, Equatable {
    public let profile: String
    public let snapshotID: String
    public let attestationID: String
    public let bundleID: String
    public let windowID: UInt32
    public let pngSHA256: String
    public let pngWidth: Int
    public let pngHeight: Int
    public let captureOriginX: Double
    public let captureOriginY: Double
    public let captureWidthPoints: Double
    public let captureHeightPoints: Double

    public init(snapshot: SnapshotEnvelope) throws {
        let captured = snapshot.captured
        guard let bundleID = captured.app.bundleIdentifier,
              (3...255).contains(bundleID.utf8.count),
              (16...128).contains(snapshot.snapshotIdentifier.utf8.count),
              (16...128).contains(snapshot.deliveryAttestationIdentifier.utf8.count),
              captured.screenshotSHA256.count == 64,
              captured.screenshotSHA256.allSatisfy({ $0.isHexDigit && !$0.isUppercase }),
              !captured.screenshotPNG.isEmpty,
              captured.screenshotPNG.count <= 900_000,
              (1...1_280).contains(captured.geometry.pngWidthPixels),
              (1...1_280).contains(captured.geometry.pngHeightPixels),
              captured.geometry.pngWidthPixels * captured.geometry.pngHeightPixels <= 1_638_400,
              captured.geometry.windowIdentifier > 0,
              captured.geometry.globalBoundsPoints.x.isFinite,
              captured.geometry.globalBoundsPoints.y.isFinite,
              captured.geometry.globalBoundsPoints.width.isFinite,
              captured.geometry.globalBoundsPoints.height.isFinite,
              captured.geometry.globalBoundsPoints.width > 0,
              captured.geometry.globalBoundsPoints.height > 0
        else {
            throw ComputerUseError.stateUnavailable("The protected snapshot metadata is incomplete.")
        }
        profile = "computer-use-v2"
        snapshotID = snapshot.snapshotIdentifier
        attestationID = snapshot.deliveryAttestationIdentifier
        self.bundleID = bundleID
        windowID = captured.geometry.windowIdentifier
        pngSHA256 = captured.screenshotSHA256
        pngWidth = captured.geometry.pngWidthPixels
        pngHeight = captured.geometry.pngHeightPixels
        let bounds = captured.geometry.globalBoundsPoints
        captureOriginX = bounds.x
        captureOriginY = bounds.y
        captureWidthPoints = bounds.width
        captureHeightPoints = bounds.height
    }

    public var json: JSONValue {
        .object([
            "profile": .string(profile),
            "snapshot_id": .string(snapshotID),
            "attestation_id": .string(attestationID),
            "bundle_id": .string(bundleID),
            "window_id": .integer(Int64(windowID)),
            "png_sha256": .string(pngSHA256),
            "png_width_px": .integer(Int64(pngWidth)),
            "png_height_px": .integer(Int64(pngHeight)),
            "capture_origin_x": .number(captureOriginX),
            "capture_origin_y": .number(captureOriginY),
            "capture_width_points": .number(captureWidthPoints),
            "capture_height_points": .number(captureHeightPoints),
        ])
    }
}

public struct ToolExecutionResult: Codable, Sendable, Equatable {
    public let text: String
    public let structuredContent: JSONValue?
    public let imagePNG: Data?
    public let isError: Bool
    public let protectedCarrier: ProtectedComputerUseCarrier?

    public init(
        text: String,
        structuredContent: JSONValue? = nil,
        imagePNG: Data? = nil,
        isError: Bool = false,
        protectedCarrier: ProtectedComputerUseCarrier? = nil
    ) {
        self.text = text
        self.structuredContent = structuredContent
        self.imagePNG = imagePNG
        self.isError = isError
        self.protectedCarrier = protectedCarrier
    }
}

public enum MouseButton: String, Codable, Sendable, CaseIterable {
    case left
    case right
    case middle
}

public struct ToolCallContext: Sendable, Equatable {
    public let clientIdentifier: String
    public let actionIdentifier: String?

    public init(clientIdentifier: String, actionIdentifier: String? = nil) {
        self.clientIdentifier = clientIdentifier
        self.actionIdentifier = actionIdentifier
    }
}

public enum ActionReceiptState: String, Codable, Sendable, Equatable {
    case prepared
    case dispatched
    case applied
    case rejected
    case outcomeUnknown = "outcome_unknown"
}

public struct ActionReceipt: Codable, Sendable, Equatable {
    public let identifier: String
    public let toolName: String
    public let snapshotIdentifier: String
    public let state: ActionReceiptState
    public let createdAt: Date
    public let updatedAt: Date
    public let failureCode: String?

    public init(
        identifier: String,
        toolName: String,
        snapshotIdentifier: String,
        state: ActionReceiptState,
        createdAt: Date,
        updatedAt: Date,
        failureCode: String? = nil
    ) {
        self.identifier = identifier
        self.toolName = toolName
        self.snapshotIdentifier = snapshotIdentifier
        self.state = state
        self.createdAt = createdAt
        self.updatedAt = updatedAt
        self.failureCode = failureCode
    }

    public func transitioned(to state: ActionReceiptState, at date: Date, failureCode: String? = nil) -> ActionReceipt {
        ActionReceipt(
            identifier: identifier,
            toolName: toolName,
            snapshotIdentifier: snapshotIdentifier,
            state: state,
            createdAt: createdAt,
            updatedAt: date,
            failureCode: failureCode
        )
    }
}
