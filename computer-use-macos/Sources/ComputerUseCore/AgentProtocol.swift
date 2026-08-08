import Foundation
import CryptoKit

public enum AgentRequestKind: String, Codable, Sendable, Equatable {
    case initialize
    case ping
    case callTool = "call_tool"
    case actionOutcome = "action_outcome"
    case attestSnapshotDelivery = "attest_snapshot_delivery"
    case invalidateSession = "invalidate_session"
    case leaseHeartbeat = "lease_heartbeat"
    case releaseOperation = "release_operation"
}

public struct AgentRequest: Codable, Sendable {
    public static let protocolVersion = 2

    public let version: Int
    public let requestIdentifier: String
    public let clientIdentifier: String
    public let kind: AgentRequestKind
    public let toolName: String?
    public let arguments: [String: JSONValue]?
    public let actionIdentifier: String?
    public let receiptIdentifier: String?
    public let sessionIdentifier: String?
    public let sequence: UInt64?
    public let authenticationTag: String?
    public let sessionSecret: String?

    public init(
        requestIdentifier: String,
        clientIdentifier: String,
        kind: AgentRequestKind,
        toolName: String? = nil,
        arguments: [String: JSONValue]? = nil,
        actionIdentifier: String? = nil,
        receiptIdentifier: String? = nil,
        sessionIdentifier: String? = nil,
        sequence: UInt64? = nil,
        authenticationTag: String? = nil,
        sessionSecret: String? = nil
    ) {
        version = Self.protocolVersion
        self.requestIdentifier = requestIdentifier
        self.clientIdentifier = clientIdentifier
        self.kind = kind
        self.toolName = toolName
        self.arguments = arguments
        self.actionIdentifier = actionIdentifier
        self.receiptIdentifier = receiptIdentifier
        self.sessionIdentifier = sessionIdentifier
        self.sequence = sequence
        self.authenticationTag = authenticationTag
        self.sessionSecret = sessionSecret
    }

    public func validate() throws {
        guard version == Self.protocolVersion,
              UUID(uuidString: requestIdentifier) != nil,
              UUID(uuidString: clientIdentifier) != nil
        else {
            throw ComputerUseError.invalidArguments("Invalid app-agent protocol envelope.")
        }

        switch kind {
        case .initialize:
            guard toolName == nil,
                  arguments == nil,
                  actionIdentifier == nil,
                  receiptIdentifier == nil,
                  sessionIdentifier == nil,
                  sequence == nil,
                  authenticationTag == nil,
                  let sessionSecret,
                  Data(base64Encoded: sessionSecret)?.count == 32
            else {
                throw ComputerUseError.invalidArguments("Invalid app-agent initialization request.")
            }
        case .ping:
            guard toolName == nil, arguments == nil, actionIdentifier == nil, receiptIdentifier == nil else {
                throw ComputerUseError.invalidArguments("Ping request contains unexpected fields.")
            }
        case .callTool:
            guard let toolName,
                  ToolCatalog.acceptedToolNames.contains(toolName),
                  arguments != nil,
                  receiptIdentifier == nil
            else {
                throw ComputerUseError.invalidArguments("Invalid call_tool request.")
            }
            if ToolCatalog.actionToolNames.contains(toolName) {
                guard let actionIdentifier,
                      !actionIdentifier.isEmpty,
                      actionIdentifier.utf8.count <= 256,
                      !actionIdentifier.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
                else {
                    throw ComputerUseError.invalidArguments("Action request has no valid action identifier.")
                }
            } else if actionIdentifier != nil {
                throw ComputerUseError.invalidArguments("Read-only request contains an action identifier.")
            }
        case .actionOutcome:
            guard let receiptIdentifier,
                  !receiptIdentifier.isEmpty,
                  receiptIdentifier.utf8.count <= 256,
                  !receiptIdentifier.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
                  toolName == nil,
                  arguments == nil,
                  actionIdentifier == nil
            else {
                throw ComputerUseError.invalidArguments("Invalid action_outcome request.")
            }
        case .attestSnapshotDelivery, .leaseHeartbeat, .releaseOperation:
            guard toolName == nil, arguments != nil, actionIdentifier == nil, receiptIdentifier == nil else {
                throw ComputerUseError.invalidArguments("Invalid app-agent lifecycle request.")
            }
        case .invalidateSession:
            guard toolName == nil,
                  arguments?.isEmpty == true,
                  actionIdentifier == nil,
                  receiptIdentifier == nil
            else {
                throw ComputerUseError.invalidArguments("Invalid session invalidation request.")
            }
        }

        if kind != .initialize {
            guard sessionSecret == nil,
                  let sessionIdentifier,
                  UUID(uuidString: sessionIdentifier) != nil,
                  let sequence,
                  sequence > 0,
                  let authenticationTag,
                  Data(base64Encoded: authenticationTag)?.count == 32
            else {
                throw ComputerUseError.invalidArguments("Missing authenticated app-agent session envelope.")
            }
        }
    }

    public func authenticated(
        sessionIdentifier: String,
        sequence: UInt64,
        key: Data
    ) throws -> AgentRequest {
        var request = AgentRequest(
            requestIdentifier: requestIdentifier,
            clientIdentifier: clientIdentifier,
            kind: kind,
            toolName: toolName,
            arguments: arguments,
            actionIdentifier: actionIdentifier,
            receiptIdentifier: receiptIdentifier,
            sessionIdentifier: sessionIdentifier,
            sequence: sequence
        )
        let tag = try AgentSessionAuthentication.tag(
            key: key,
            domain: "request-v2",
            payload: request.authenticationPayload()
        )
        request = AgentRequest(
            requestIdentifier: requestIdentifier,
            clientIdentifier: clientIdentifier,
            kind: kind,
            toolName: toolName,
            arguments: arguments,
            actionIdentifier: actionIdentifier,
            receiptIdentifier: receiptIdentifier,
            sessionIdentifier: sessionIdentifier,
            sequence: sequence,
            authenticationTag: tag
        )
        return request
    }

    public func authenticationPayload() throws -> Data {
        try JSONEncoder.agentWire.encode(AgentRequestAuthenticationPayload(self))
    }
}

public struct AgentProtocolError: Codable, Sendable, Equatable {
    public let code: String
    public let message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }
}

public struct AgentResponse: Codable, Sendable {
    public let version: Int
    public let requestIdentifier: String
    public let toolResult: ToolExecutionResult?
    public let receipt: ActionReceipt?
    public let pong: Bool?
    public let error: AgentProtocolError?
    public let sessionIdentifier: String?
    public let sequence: UInt64?
    public let authenticationTag: String?

    public init(
        requestIdentifier: String,
        toolResult: ToolExecutionResult? = nil,
        receipt: ActionReceipt? = nil,
        pong: Bool? = nil,
        error: AgentProtocolError? = nil,
        sessionIdentifier: String? = nil,
        sequence: UInt64? = nil,
        authenticationTag: String? = nil
    ) {
        version = AgentRequest.protocolVersion
        self.requestIdentifier = requestIdentifier
        self.toolResult = toolResult
        self.receipt = receipt
        self.pong = pong
        self.error = error
        self.sessionIdentifier = sessionIdentifier
        self.sequence = sequence
        self.authenticationTag = authenticationTag
    }

    public func authenticated(sessionIdentifier: String, sequence: UInt64, key: Data) throws -> AgentResponse {
        let response = AgentResponse(
            requestIdentifier: requestIdentifier,
            toolResult: toolResult,
            receipt: receipt,
            pong: pong,
            error: error,
            sessionIdentifier: sessionIdentifier,
            sequence: sequence
        )
        let tag = try AgentSessionAuthentication.tag(
            key: key,
            domain: "response-v2",
            payload: response.authenticationPayload()
        )
        return AgentResponse(
            requestIdentifier: requestIdentifier,
            toolResult: toolResult,
            receipt: receipt,
            pong: pong,
            error: error,
            sessionIdentifier: sessionIdentifier,
            sequence: sequence,
            authenticationTag: tag
        )
    }

    public func authenticationPayload() throws -> Data {
        try JSONEncoder.agentWire.encode(AgentResponseAuthenticationPayload(self))
    }
}

public enum AgentSessionAuthentication {
    public static func tag(key: Data, domain: String, payload: Data) throws -> String {
        guard key.count == 32 else {
            throw ComputerUseError.invalidArguments("Invalid app-agent session key.")
        }
        var authenticated = Data(domain.utf8)
        authenticated.append(0)
        authenticated.append(payload)
        return Data(HMAC<SHA256>.authenticationCode(
            for: authenticated,
            using: SymmetricKey(data: key)
        )).base64EncodedString()
    }

    public static func verify(tag: String, key: Data, domain: String, payload: Data) -> Bool {
        guard let supplied = Data(base64Encoded: tag),
              let expected = try? self.tag(key: key, domain: domain, payload: payload),
              let expectedData = Data(base64Encoded: expected),
              supplied.count == expectedData.count
        else { return false }
        return zip(supplied, expectedData).reduce(UInt8(0)) { $0 | ($1.0 ^ $1.1) } == 0
    }

    public static func initializationProof(
        key: Data,
        clientIdentifier: String,
        requestIdentifier: String,
        sessionIdentifier: String
    ) throws -> String {
        try tag(
            key: key,
            domain: "initialize-v2",
            payload: Data("\(clientIdentifier)\u{1f}\(requestIdentifier)\u{1f}\(sessionIdentifier)".utf8)
        )
    }

    public static func verifyInitializationProof(
        _ proof: String,
        key: Data,
        clientIdentifier: String,
        requestIdentifier: String,
        sessionIdentifier: String
    ) -> Bool {
        verify(
            tag: proof,
            key: key,
            domain: "initialize-v2",
            payload: Data("\(clientIdentifier)\u{1f}\(requestIdentifier)\u{1f}\(sessionIdentifier)".utf8)
        )
    }
}

private struct AgentRequestAuthenticationPayload: Codable {
    let version: Int
    let requestIdentifier: String
    let clientIdentifier: String
    let kind: AgentRequestKind
    let toolName: String?
    let arguments: [String: JSONValue]?
    let actionIdentifier: String?
    let receiptIdentifier: String?
    let sessionIdentifier: String?
    let sequence: UInt64?

    init(_ request: AgentRequest) {
        version = request.version
        requestIdentifier = request.requestIdentifier
        clientIdentifier = request.clientIdentifier
        kind = request.kind
        toolName = request.toolName
        arguments = request.arguments
        actionIdentifier = request.actionIdentifier
        receiptIdentifier = request.receiptIdentifier
        sessionIdentifier = request.sessionIdentifier
        sequence = request.sequence
    }
}

private struct AgentResponseAuthenticationPayload: Codable {
    let version: Int
    let requestIdentifier: String
    let toolResult: ToolExecutionResult?
    let receipt: ActionReceipt?
    let pong: Bool?
    let error: AgentProtocolError?
    let sessionIdentifier: String?
    let sequence: UInt64?

    init(_ response: AgentResponse) {
        version = response.version
        requestIdentifier = response.requestIdentifier
        toolResult = response.toolResult
        receipt = response.receipt
        pong = response.pong
        error = response.error
        sessionIdentifier = response.sessionIdentifier
        sequence = response.sequence
    }
}

public enum AgentWire {
    // A maximum-sized protected PNG expands to about 1.2 MB in JSON base64.
    // Keep enough headroom for the observation envelope without allowing each
    // authenticated connection to reserve tens of megabytes from one header.
    public static let maximumFrameBytes = 2 * 1_024 * 1_024

    public static func encodeFrame<T: Encodable>(_ value: T) throws -> Data {
        let payload = try JSONEncoder.agentWire.encode(value)
        guard payload.count <= maximumFrameBytes else {
            throw ComputerUseError.stateUnavailable("App-agent response exceeds the wire limit.")
        }
        var length = UInt64(payload.count).bigEndian
        var frame = withUnsafeBytes(of: &length) { Data($0) }
        frame.append(payload)
        return frame
    }

    public static func decodeLength(_ data: Data) throws -> Int {
        guard data.count == MemoryLayout<UInt64>.size else {
            throw ComputerUseError.invalidArguments("Invalid app-agent frame header.")
        }
        let raw = data.withUnsafeBytes { bytes in
            bytes.loadUnaligned(as: UInt64.self)
        }
        let length = UInt64(bigEndian: raw)
        guard length > 0, length <= UInt64(maximumFrameBytes), let result = Int(exactly: length) else {
            throw ComputerUseError.invalidArguments("Invalid app-agent frame length.")
        }
        return result
    }

    public static func decodeRequest(_ data: Data) throws -> AgentRequest {
        try requireObjectKeys(
            data,
            allowed: [
                "version", "requestIdentifier", "clientIdentifier", "kind", "toolName",
                "arguments", "actionIdentifier", "receiptIdentifier", "sessionIdentifier",
                "sequence", "authenticationTag", "sessionSecret",
            ],
            required: ["version", "requestIdentifier", "clientIdentifier", "kind"]
        )
        let request = try JSONDecoder.agentWire.decode(AgentRequest.self, from: data)
        try request.validate()
        return request
    }

    public static func decodeResponse(_ data: Data) throws -> AgentResponse {
        try requireObjectKeys(
            data,
            allowed: [
                "version", "requestIdentifier", "toolResult", "receipt", "pong", "error",
                "sessionIdentifier", "sequence", "authenticationTag",
            ],
            required: ["version", "requestIdentifier"]
        )
        let response = try JSONDecoder.agentWire.decode(AgentResponse.self, from: data)
        guard response.version == AgentRequest.protocolVersion,
              UUID(uuidString: response.requestIdentifier) != nil
        else {
            throw ComputerUseError.invalidArguments("Invalid app-agent response envelope.")
        }
        return response
    }

    private static func requireObjectKeys(_ data: Data, allowed: Set<String>, required: Set<String>) throws {
        let value = try JSONDecoder().decode(JSONValue.self, from: data)
        guard let object = value.objectValue,
              Set(object.keys).isSubset(of: allowed),
              required.isSubset(of: Set(object.keys))
        else {
            throw ComputerUseError.invalidArguments("Invalid app-agent wire schema.")
        }
    }
}

public enum ComputerUsePaths {
    public static let bundleIdentifier = "com.xai.grok.computer-use"
    public static let appBundleName = "Grok Computer Use.app"
    public static let appExecutableName = "GrokComputerUseApp"
    public static let relayExecutableName = "grok-computer-use-mcp"

    public static func supportDirectory(fileManager: FileManager = .default) throws -> URL {
        guard let base = fileManager.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            throw ComputerUseError.stateUnavailable("The user Application Support directory is unavailable.")
        }
        return base.appendingPathComponent(bundleIdentifier, isDirectory: true)
    }

    public static func socketURL(fileManager: FileManager = .default) throws -> URL {
        try supportDirectory(fileManager: fileManager).appendingPathComponent("agent-v2.sock", isDirectory: false)
    }

    public static func receiptsDirectory(fileManager: FileManager = .default) throws -> URL {
        try supportDirectory(fileManager: fileManager).appendingPathComponent("Receipts", isDirectory: true)
    }

    public static func installedAppURL(fileManager: FileManager = .default) -> URL {
        fileManager.homeDirectoryForCurrentUser
            .appendingPathComponent("Applications", isDirectory: true)
            .appendingPathComponent(appBundleName, isDirectory: true)
    }
}

public extension JSONEncoder {
    static var agentWire: JSONEncoder {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        return encoder
    }
}

public extension JSONDecoder {
    static var agentWire: JSONDecoder {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return decoder
    }
}
