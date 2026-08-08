import Foundation

public actor MCPServer {
    private static let supportedProtocolVersions = ["2025-06-18", "2025-03-26"]
    private static let maximumLineBytes = AgentWire.maximumFrameBytes

    private let caller: any ToolCalling
    private let clientIdentifier: String
    private var initialized = false

    public init(caller: any ToolCalling, clientIdentifier: String = UUID().uuidString.lowercased()) {
        self.caller = caller
        self.clientIdentifier = clientIdentifier
    }

    public func handle(line: String) async -> String? {
        guard let data = line.data(using: .utf8), data.count <= Self.maximumLineBytes else {
            return encode(errorResponse(id: .null, code: -32_600, message: "Invalid Request"))
        }

        let value: JSONValue
        do {
            value = try JSONDecoder().decode(JSONValue.self, from: data)
        } catch {
            return encode(errorResponse(id: .null, code: -32_700, message: "Parse error"))
        }

        do {
            let request = try parseRequest(value)
            let response = try await dispatch(request)
            return response.map(encode)
        } catch let failure as JSONRPCFailure {
            return encode(errorResponse(id: failure.id, code: failure.code, message: failure.message))
        } catch {
            return encode(errorResponse(id: requestID(from: value), code: -32_603, message: "Internal error"))
        }
    }

    public func disconnect() async {
        await caller.clientDisconnected(identifier: clientIdentifier)
    }

    private func dispatch(_ request: JSONRPCRequest) async throws -> JSONValue? {
        switch request.method {
        case "initialize":
            guard let id = request.id else { throw JSONRPCFailure(id: .null, code: -32_600, message: "Invalid Request") }
            let params = try requiredObject(request.params, id: id)
            try requireAllowedKeys(params, allowed: ["protocolVersion", "capabilities", "clientInfo", "_meta"], id: id)
            guard let requested = params["protocolVersion"]?.stringValue,
                  params["capabilities"]?.objectValue != nil,
                  params["clientInfo"]?.objectValue != nil
            else {
                throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
            }
            let selected = Self.supportedProtocolVersions.contains(requested)
                ? requested
                : Self.supportedProtocolVersions[0]
            initialized = true
            return successResponse(id: id, result: .object([
                "protocolVersion": .string(selected),
                "capabilities": .object([
                    "tools": .object(["listChanged": .bool(false)]),
                ]),
                "serverInfo": .object([
                    "name": .string("grok-computer-use-macos"),
                    "version": .string("0.1.0"),
                ]),
            ]))

        case "notifications/initialized":
            guard request.id == nil else {
                throw JSONRPCFailure(id: request.id ?? .null, code: -32_600, message: "Invalid Request")
            }
            try requireEmptyOrMetaParams(request.params, id: .null)
            return nil

        case "ping":
            guard let id = request.id else { throw JSONRPCFailure(id: .null, code: -32_600, message: "Invalid Request") }
            try requireEmptyOrMetaParams(request.params, id: id)
            return successResponse(id: id, result: .object([:]))

        case "tools/list":
            guard initialized, let id = request.id else {
                throw JSONRPCFailure(id: request.id ?? .null, code: -32_600, message: "Invalid Request")
            }
            let params = try optionalObject(request.params, id: id)
            try requireAllowedKeys(params, allowed: ["cursor", "_meta"], id: id)
            guard params["cursor"] == nil || params["cursor"] == .null else {
                throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
            }
            return successResponse(id: id, result: .object([
                "tools": .array(ToolCatalog.all.map(\.json)),
            ]))

        case "tools/call":
            guard initialized, let id = request.id else {
                throw JSONRPCFailure(id: request.id ?? .null, code: -32_600, message: "Invalid Request")
            }
            let params = try requiredObject(request.params, id: id)
            try requireAllowedKeys(params, allowed: ["name", "arguments", "_meta"], id: id)
            guard let name = params["name"]?.stringValue,
                  ToolCatalog.acceptedToolNames.contains(name)
            else {
                throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
            }
            let arguments: [String: JSONValue]
            if let rawArguments = params["arguments"] {
                guard let object = rawArguments.objectValue else {
                    throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
                }
                arguments = object
            } else {
                arguments = [:]
            }
            let invocation = try parseTrustedInvocation(params["_meta"], toolName: name, id: id)
            let actionID = ToolCatalog.actionToolNames.contains(name) ? invocation.actionIdentifier : nil
            let result = await caller.callTool(
                name: name,
                arguments: arguments,
                context: ToolCallContext(clientIdentifier: clientIdentifier, actionIdentifier: actionID)
            )
            return successResponse(id: id, result: toolCallResult(result))

        default:
            guard let id = request.id else { return nil }
            throw JSONRPCFailure(id: id, code: -32_601, message: "Method not found")
        }
    }

    private func parseRequest(_ value: JSONValue) throws -> JSONRPCRequest {
        guard let object = value.objectValue else {
            throw JSONRPCFailure(id: .null, code: -32_600, message: "Invalid Request")
        }
        let id = validID(object["id"])
        guard Set(object.keys).isSubset(of: ["jsonrpc", "id", "method", "params"]),
              object["jsonrpc"] == .string("2.0"),
              let method = object["method"]?.stringValue,
              !method.isEmpty,
              object["id"] == nil || id != nil
        else {
            throw JSONRPCFailure(id: id ?? .null, code: -32_600, message: "Invalid Request")
        }
        if let params = object["params"], params.objectValue == nil && !isArray(params) {
            throw JSONRPCFailure(id: id ?? .null, code: -32_602, message: "Invalid params")
        }
        return JSONRPCRequest(id: id, method: method, params: object["params"])
    }

    private func validID(_ value: JSONValue?) -> JSONValue? {
        guard let value else { return nil }
        switch value {
        case .string, .integer: return value
        default: return nil
        }
    }

    private func isArray(_ value: JSONValue) -> Bool {
        if case .array = value { return true }
        return false
    }

    private func requiredObject(_ value: JSONValue?, id: JSONValue) throws -> [String: JSONValue] {
        guard let object = value?.objectValue else {
            throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
        }
        return object
    }

    private func optionalObject(_ value: JSONValue?, id: JSONValue) throws -> [String: JSONValue] {
        guard let value else { return [:] }
        guard let object = value.objectValue else {
            throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
        }
        return object
    }

    private func requireAllowedKeys(_ object: [String: JSONValue], allowed: Set<String>, id: JSONValue) throws {
        guard Set(object.keys).isSubset(of: allowed) else {
            throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
        }
    }

    private func requireEmptyOrMetaParams(_ params: JSONValue?, id: JSONValue) throws {
        let object = try optionalObject(params, id: id)
        try requireAllowedKeys(object, allowed: ["_meta"], id: id)
    }

    private func parseTrustedInvocation(
        _ value: JSONValue?,
        toolName: String,
        id: JSONValue
    ) throws -> TrustedInvocation {
        guard let root = value?.objectValue,
              Set(root.keys) == ["xai/computer-use-v2"],
              let payload = root["xai/computer-use-v2"]?.objectValue,
              Set(payload.keys) == [
                "profile", "logical_call_id", "session_id", "workflow_id", "action_id", "tool_name",
              ],
              payload["profile"] == .string("computer-use-v2"),
              payload["tool_name"] == .string(toolName),
              let logicalCallID = payload["logical_call_id"]?.stringValue,
              let sessionID = payload["session_id"]?.stringValue,
              let workflowID = payload["workflow_id"]?.stringValue,
              let actionID = payload["action_id"]?.stringValue,
              [logicalCallID, sessionID, workflowID, actionID].allSatisfy(validTrustedIdentity)
        else {
            throw JSONRPCFailure(id: id, code: -32_602, message: "Invalid params")
        }
        return TrustedInvocation(actionIdentifier: actionID)
    }

    private func validTrustedIdentity(_ value: String) -> Bool {
        !value.isEmpty
            && value.utf8.count <= 256
            && !value.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
    }

    private func toolCallResult(_ result: ToolExecutionResult) -> JSONValue {
        if let carrier = result.protectedCarrier {
            guard let image = result.imagePNG,
                  result.text.utf8.count <= 16 * 1_024,
                  image.count <= 900_000,
                  !result.isError,
                  result.structuredContent == nil
            else {
                return .object([
                    "content": .array([
                        .object([
                            "type": .string("text"),
                            "text": .string("Protected computer-use observation rejected by the relay."),
                        ]),
                    ]),
                    "isError": .bool(true),
                ])
            }
            return .object([
                "content": .array([
                    .object(["type": .string("text"), "text": .string(result.text)]),
                    .object([
                        "type": .string("image"),
                        "mimeType": .string("image/png"),
                        "data": .string(image.base64EncodedString()),
                    ]),
                ]),
                "isError": .bool(false),
                "_meta": .object([
                    "xai/computer-use-v2": carrier.json,
                ]),
            ])
        }

        var content: [JSONValue] = [
            .object(["type": .string("text"), "text": .string(result.text)]),
        ]
        if result.imagePNG != nil {
            return .object([
                "content": .array([
                    .object([
                        "type": .string("text"),
                        "text": .string("Unattested computer-use image rejected by the relay."),
                    ]),
                ]),
                "isError": .bool(true),
            ])
        }
        var response: [String: JSONValue] = [
            "content": .array(content),
            "isError": .bool(result.isError),
        ]
        if let structuredContent = result.structuredContent {
            response["structuredContent"] = structuredContent
        }
        return .object(response)
    }

    private func successResponse(id: JSONValue, result: JSONValue) -> JSONValue {
        .object(["jsonrpc": .string("2.0"), "id": id, "result": result])
    }

    private func errorResponse(id: JSONValue, code: Int64, message: String) -> JSONValue {
        .object([
            "jsonrpc": .string("2.0"),
            "id": id,
            "error": .object(["code": .integer(code), "message": .string(message)]),
        ])
    }

    private func requestID(from value: JSONValue) -> JSONValue {
        guard let raw = value.objectValue?["id"], let id = validID(raw) else { return .null }
        return id
    }

    private func encode(_ value: JSONValue) -> String {
        guard let data = try? JSONEncoder().encode(value), let string = String(data: data, encoding: .utf8) else {
            return "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32603,\"message\":\"Internal error\"}}"
        }
        return string
    }
}

private struct JSONRPCRequest {
    let id: JSONValue?
    let method: String
    let params: JSONValue?
}

private struct JSONRPCFailure: Error {
    let id: JSONValue
    let code: Int64
    let message: String
}

private struct TrustedInvocation {
    let actionIdentifier: String
}
