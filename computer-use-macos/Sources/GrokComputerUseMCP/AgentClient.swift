import ComputerUseCore
import Darwin
import Foundation
import Security

actor AgentClient: ToolCalling {
    private let clientIdentifier: String
    private let socketURL: URL
    private let appURL: URL
    private let expectedAppExecutableURL: URL
    private let relaySigningIdentity: ProcessSigningIdentity
    private var descriptor: Int32?
    private var sessionIdentifier: String?
    private var sessionKey: Data?
    private var nextSequence: UInt64 = 1

    init(clientIdentifier: String) throws {
        guard UUID(uuidString: clientIdentifier) != nil else {
            throw ComputerUseError.internalFailure("Invalid relay client identity.")
        }
        self.clientIdentifier = clientIdentifier
        socketURL = try ComputerUsePaths.socketURL()
        appURL = ComputerUsePaths.installedAppURL()
        expectedAppExecutableURL = appURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("MacOS", isDirectory: true)
            .appendingPathComponent(ComputerUsePaths.appExecutableName, isDirectory: false)
            .resolvingSymlinksInPath()
            .standardizedFileURL
        let relayCode = try Self.currentCode()
        let relayExecutableURL = try Self.executableURL(for: relayCode)
        let relayIdentity = try Self.signingIdentity(
            relayCode,
            executableBasename: relayExecutableURL.lastPathComponent
        )
        guard GrokComponentSigningPolicy.isTrustedRelay(relayIdentity) else {
            throw ComputerUseError.permissionDenied("The MCP relay code signature is not trusted.")
        }
        try Self.requireAppleSignature(
            code: relayCode,
            identifier: GrokComponentSigningPolicy.relayIdentifier,
            teamIdentifier: relayIdentity.teamIdentifier
        )
        relaySigningIdentity = relayIdentity
    }

    deinit {
        if let descriptor {
            shutdown(descriptor, SHUT_RDWR)
            close(descriptor)
        }
    }

    func callTool(name: String, arguments: [String: JSONValue], context: ToolCallContext) async -> ToolExecutionResult {
        guard context.clientIdentifier == clientIdentifier else {
            return transportError("Relay client identity mismatch.")
        }
        let kind: AgentRequestKind
        switch name {
        case "attest_snapshot_delivery": kind = .attestSnapshotDelivery
        case "invalidate_session": kind = .invalidateSession
        case "lease_heartbeat": kind = .leaseHeartbeat
        case "release_operation": kind = .releaseOperation
        default: kind = .callTool
        }
        let request = AgentRequest(
            requestIdentifier: UUID().uuidString.lowercased(),
            clientIdentifier: clientIdentifier,
            kind: kind,
            toolName: kind == .callTool ? name : nil,
            arguments: arguments,
            actionIdentifier: context.actionIdentifier
        )

        do {
            return try exchange(request).toolResult ?? transportError("The app agent returned no tool result.")
        } catch {
            closeConnection()
            switch AgentTransportRecoveryPolicy.disposition(
                kind: kind,
                toolName: name,
                actionIdentifier: context.actionIdentifier
            ) {
            case .retryReadOnlyCall:
                do {
                    return try exchange(request).toolResult ?? transportError("The app agent returned no tool result.")
                } catch {
                    closeConnection()
                    return transportError("The app agent is unavailable.")
                }
            case let .recoverAction(actionIdentifier):
                return recoverActionOutcome(actionIdentifier)
            case .failClosed:
                return transportError("The app-agent transport failed; this lifecycle operation was not retried.")
            }
        }
    }

    func actionOutcome(identifier: String) async -> ActionReceipt? {
        queryOutcome(identifier)
    }

    func clientDisconnected(identifier: String) async {
        if identifier == clientIdentifier { closeConnection() }
    }

    private func recoverActionOutcome(_ identifier: String) -> ToolExecutionResult {
        guard let receipt = queryOutcome(identifier) else {
            return ToolExecutionResult(
                text: "The transport failed before a durable action outcome was available. The action was not retried; call get_app_state.",
                isError: true
            )
        }

        switch receipt.state {
        case .applied:
            return ToolExecutionResult(
                text: "The action was durably recorded as applied and was not retried. Call get_app_state for a fresh snapshot."
            )
        case .rejected:
            return ToolExecutionResult(
                text: "The action was durably rejected and was not retried. Call get_app_state before another action.",
                isError: true
            )
        case .prepared:
            return ToolExecutionResult(
                text: "The action was prepared but not durably dispatched. It was not retried; call get_app_state.",
                isError: true
            )
        case .dispatched, .outcomeUnknown:
            return ToolExecutionResult(
                text: "The action outcome is unknown and it will never be retried. Call get_app_state to observe the desktop.",
                isError: true
            )
        }
    }

    private func queryOutcome(_ identifier: String) -> ActionReceipt? {
        closeConnection()
        let request = AgentRequest(
            requestIdentifier: UUID().uuidString.lowercased(),
            clientIdentifier: clientIdentifier,
            kind: .actionOutcome,
            receiptIdentifier: identifier
        )
        do {
            return try exchange(request).receipt
        } catch {
            closeConnection()
            return nil
        }
    }

    private func exchange(_ request: AgentRequest) throws -> AgentResponse {
        let socket = try connectedSocket()
        guard let sessionIdentifier, let sessionKey, nextSequence < UInt64.max else {
            throw ComputerUseError.stateUnavailable("The app-agent session is unavailable.")
        }
        let sequence = nextSequence
        nextSequence += 1
        let authenticatedRequest = try request.authenticated(
            sessionIdentifier: sessionIdentifier,
            sequence: sequence,
            key: sessionKey
        )
        try writeAll(AgentWire.encodeFrame(authenticatedRequest), to: socket)
        guard let header = try readExactly(count: MemoryLayout<UInt64>.size, from: socket) else {
            throw ComputerUseError.stateUnavailable("The app agent closed the connection.")
        }
        let length = try AgentWire.decodeLength(header)
        guard let payload = try readExactly(count: length, from: socket) else {
            throw ComputerUseError.stateUnavailable("The app agent returned an incomplete response.")
        }
        let response = try AgentWire.decodeResponse(payload)
        guard response.requestIdentifier == request.requestIdentifier,
              response.sessionIdentifier == sessionIdentifier,
              response.sequence == sequence,
              let tag = response.authenticationTag,
              AgentSessionAuthentication.verify(
                tag: tag,
                key: sessionKey,
                domain: "response-v2",
                payload: try response.authenticationPayload()
              )
        else {
            throw ComputerUseError.stateUnavailable("The app-agent response identifier did not match.")
        }
        if let error = response.error {
            throw ComputerUseError.stateUnavailable(error.message)
        }
        return response
    }

    private func connectedSocket() throws -> Int32 {
        if let descriptor { return descriptor }
        try verifyInstalledBundleExists()
        if let connected = tryConnect() {
            do {
                try initializeSession(on: connected)
            } catch {
                shutdown(connected, SHUT_RDWR)
                close(connected)
                throw error
            }
            descriptor = connected
            return connected
        }
        try launchApp()
        let deadline = Date().addingTimeInterval(10)
        while Date() < deadline {
            if let connected = tryConnect() {
                do {
                    try initializeSession(on: connected)
                } catch {
                    shutdown(connected, SHUT_RDWR)
                    close(connected)
                    throw error
                }
                descriptor = connected
                return connected
            }
            usleep(50_000)
        }
        throw ComputerUseError.stateUnavailable("Timed out waiting for Grok Computer Use.app.")
    }

    private func tryConnect() -> Int32? {
        guard (try? verifyPrivateSocketPath()) != nil else { return nil }
        let socket = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard socket >= 0 else { return nil }
        do {
            try configureRelaySocket(socket)
            var address = try relayUnixAddress(path: socketURL.path)
            let result = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.connect(socket, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else {
                close(socket)
                return nil
            }
            try verifyServerPeer(socket)
            return socket
        } catch {
            shutdown(socket, SHUT_RDWR)
            close(socket)
            return nil
        }
    }

    private func verifyInstalledBundleExists() throws {
        var isDirectory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: appURL.path, isDirectory: &isDirectory), isDirectory.boolValue,
              FileManager.default.isExecutableFile(atPath: expectedAppExecutableURL.path)
        else {
            throw ComputerUseError.stateUnavailable("Install Grok Computer Use.app in ~/Applications before using the MCP relay.")
        }
    }

    private func verifyPrivateSocketPath() throws {
        let directory = socketURL.deletingLastPathComponent()
        var directoryStatus = stat()
        var socketStatus = stat()
        guard lstat(directory.path, &directoryStatus) == 0,
              directoryStatus.st_uid == geteuid(),
              (directoryStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR),
              (directoryStatus.st_mode & 0o077) == 0,
              lstat(socketURL.path, &socketStatus) == 0,
              socketStatus.st_uid == geteuid(),
              (socketStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK),
              (socketStatus.st_mode & 0o077) == 0
        else {
            throw ComputerUseError.permissionDenied("The app-agent socket path is not private and user-owned.")
        }
    }

    private func verifyServerPeer(_ socket: Int32) throws {
        try verifyInstalledBundleSeal()
        var peerUID = uid_t.max
        var peerGID = gid_t.max
        guard getpeereid(socket, &peerUID, &peerGID) == 0,
              peerUID == geteuid()
        else {
            throw ComputerUseError.permissionDenied("The app-agent server identity is not trusted.")
        }
        let appCode = try Self.guestCode(auditToken: peerAuditToken(socket))
        let pathBeforeValidation = try Self.executableURL(for: appCode)
        guard pathBeforeValidation == expectedAppExecutableURL else {
            throw ComputerUseError.permissionDenied("The app-agent server identity is not trusted.")
        }
        try Self.requireAppleSignature(
            code: appCode,
            identifier: GrokComponentSigningPolicy.appIdentifier,
            teamIdentifier: relaySigningIdentity.teamIdentifier
        )
        let appIdentity = try Self.signingIdentity(
            appCode,
            executableBasename: pathBeforeValidation.lastPathComponent
        )
        let pathAfterValidation = try Self.executableURL(for: appCode)
        guard pathAfterValidation == pathBeforeValidation,
              GrokComponentSigningPolicy.accepts(app: appIdentity, relay: relaySigningIdentity)
        else {
            throw ComputerUseError.permissionDenied("The app-agent server identity is not trusted.")
        }
    }

    private func verifyInstalledBundleSeal() throws {
        var appCode: SecStaticCode?
        let flags = SecCSFlags(rawValue: 0)
        let requirement = try Self.appleRequirement(
            identifier: GrokComponentSigningPolicy.appIdentifier,
            teamIdentifier: relaySigningIdentity.teamIdentifier
        )
        guard SecStaticCodeCreateWithPath(appURL as CFURL, flags, &appCode) == errSecSuccess,
              let appCode,
              SecStaticCodeCheckValidity(appCode, flags, requirement) == errSecSuccess,
              let identity = try? Self.staticSigningIdentity(
                  appCode,
                  executableBasename: expectedAppExecutableURL.lastPathComponent
              ),
              GrokComponentSigningPolicy.accepts(app: identity, relay: relaySigningIdentity)
        else {
            throw ComputerUseError.permissionDenied("The installed app bundle seal is invalid.")
        }
    }

    private func peerAuditToken(_ socket: Int32) throws -> audit_token_t {
        var token = audit_token_t()
        var tokenSize = socklen_t(MemoryLayout<audit_token_t>.size)
        guard getsockopt(socket, SOL_LOCAL, LOCAL_PEERTOKEN, &token, &tokenSize) == 0,
              tokenSize == MemoryLayout<audit_token_t>.size
        else {
            throw ComputerUseError.permissionDenied("The app-agent server has no stable process credential.")
        }
        return token
    }

    private static func currentCode() throws -> SecCode {
        var code: SecCode?
        guard SecCodeCopySelf(SecCSFlags(rawValue: 0), &code) == errSecSuccess, let code else {
            throw ComputerUseError.permissionDenied("The process code signature is unavailable.")
        }
        return code
    }

    private static func guestCode(auditToken: audit_token_t) throws -> SecCode {
        let tokenData = withUnsafeBytes(of: auditToken) { Data($0) }
        let attributes = [
            kSecGuestAttributeAudit as String: tokenData,
        ] as CFDictionary
        var code: SecCode?
        guard SecCodeCopyGuestWithAttributes(
            nil,
            attributes,
            SecCSFlags(rawValue: 0),
            &code
        ) == errSecSuccess, let code else {
            throw ComputerUseError.permissionDenied("The process code signature is unavailable.")
        }
        return code
    }

    private static func executableURL(for code: SecCode) throws -> URL {
        var rawInformation: CFDictionary?
        guard SecCodeCopySigningInformation(
            code,
            SecCSFlags(rawValue: kSecCSSigningInformation),
            &rawInformation
        ) == errSecSuccess,
              let information = rawInformation as? [String: Any],
              let executableURL = information[kSecCodeInfoMainExecutable as String] as? URL
        else {
            throw ComputerUseError.permissionDenied("The process executable path is unavailable.")
        }
        return executableURL.resolvingSymlinksInPath().standardizedFileURL
    }

    private static func requireAppleSignature(
        code: SecCode,
        identifier: String,
        teamIdentifier: String
    ) throws {
        let requirement = try appleRequirement(identifier: identifier, teamIdentifier: teamIdentifier)
        guard SecCodeCheckValidity(code, SecCSFlags(rawValue: 0), requirement) == errSecSuccess else {
            throw ComputerUseError.permissionDenied("The process code signature is not trusted.")
        }
    }

    private static func appleRequirement(identifier: String, teamIdentifier: String) throws -> SecRequirement {
        guard [
            GrokComponentSigningPolicy.appIdentifier,
            GrokComponentSigningPolicy.relayIdentifier,
        ].contains(identifier),
        GrokHostSigningPolicy.isValidTeamIdentifier(teamIdentifier)
        else {
            throw ComputerUseError.permissionDenied("The component signing requirement is invalid.")
        }
        let expression = "anchor apple generic and identifier \"\(identifier)\" and certificate leaf[subject.OU] = \"\(teamIdentifier)\""
        var requirement: SecRequirement?
        guard SecRequirementCreateWithString(
            expression as CFString,
            SecCSFlags(rawValue: 0),
            &requirement
        ) == errSecSuccess, let requirement else {
            throw ComputerUseError.permissionDenied("The component signing requirement is unavailable.")
        }
        return requirement
    }

    private static func signingIdentity(
        _ code: SecCode,
        executableBasename: String
    ) throws -> ProcessSigningIdentity {
        var rawInformation: CFDictionary?
        guard SecCodeCopySigningInformation(
            code,
            SecCSFlags(rawValue: kSecCSSigningInformation),
            &rawInformation
        ) == errSecSuccess else {
            throw ComputerUseError.permissionDenied("The process signing identity is unavailable.")
        }
        return try signingIdentity(
            information: rawInformation,
            executableBasename: executableBasename
        )
    }

    private static func staticSigningIdentity(
        _ code: SecStaticCode,
        executableBasename: String
    ) throws -> ProcessSigningIdentity {
        var rawInformation: CFDictionary?
        guard SecCodeCopySigningInformation(
            code,
            SecCSFlags(rawValue: kSecCSSigningInformation),
            &rawInformation
        ) == errSecSuccess else {
            throw ComputerUseError.permissionDenied("The installed component signing identity is unavailable.")
        }
        return try signingIdentity(
            information: rawInformation,
            executableBasename: executableBasename
        )
    }

    private static func signingIdentity(
        information rawInformation: CFDictionary?,
        executableBasename: String
    ) throws -> ProcessSigningIdentity {
        guard let information = rawInformation as? [String: Any],
              let identifier = information[kSecCodeInfoIdentifier as String] as? String,
              let teamIdentifier = information[kSecCodeInfoTeamIdentifier as String] as? String
        else {
            throw ComputerUseError.permissionDenied("The component signing identity is incomplete.")
        }
        return ProcessSigningIdentity(
            identifier: identifier,
            teamIdentifier: teamIdentifier,
            executableBasename: executableBasename
        )
    }

    private func launchApp() throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/open")
        process.arguments = ["-gj", appURL.path]
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            throw ComputerUseError.stateUnavailable("LaunchServices could not start Grok Computer Use.app.")
        }
    }

    private func closeConnection() {
        guard let descriptor else { return }
        self.descriptor = nil
        sessionIdentifier = nil
        sessionKey = nil
        nextSequence = 1
        shutdown(descriptor, SHUT_RDWR)
        close(descriptor)
    }

    private func initializeSession(on socket: Int32) throws {
        var key = Data(count: 32)
        guard key.withUnsafeMutableBytes({
            SecRandomCopyBytes(kSecRandomDefault, $0.count, $0.baseAddress!)
        }) == errSecSuccess else {
            throw ComputerUseError.stateUnavailable("Could not create an app-agent session key.")
        }
        let request = AgentRequest(
            requestIdentifier: UUID().uuidString.lowercased(),
            clientIdentifier: clientIdentifier,
            kind: .initialize,
            sessionSecret: key.base64EncodedString()
        )
        try writeAll(AgentWire.encodeFrame(request), to: socket)
        guard let header = try readExactly(count: MemoryLayout<UInt64>.size, from: socket) else {
            throw ComputerUseError.stateUnavailable("The app agent closed during session initialization.")
        }
        let length = try AgentWire.decodeLength(header)
        guard let payload = try readExactly(count: length, from: socket) else {
            throw ComputerUseError.stateUnavailable("The app agent returned an incomplete initialization response.")
        }
        let response = try AgentWire.decodeResponse(payload)
        guard response.requestIdentifier == request.requestIdentifier,
              response.pong == true,
              let sessionIdentifier = response.sessionIdentifier,
              UUID(uuidString: sessionIdentifier) != nil,
              let proof = response.authenticationTag,
              AgentSessionAuthentication.verifyInitializationProof(
                proof,
                key: key,
                clientIdentifier: clientIdentifier,
                requestIdentifier: request.requestIdentifier,
                sessionIdentifier: sessionIdentifier
              )
        else {
            throw ComputerUseError.permissionDenied("The app-agent session handshake failed.")
        }
        self.sessionIdentifier = sessionIdentifier
        sessionKey = key
        nextSequence = 1
    }

    private func readExactly(count: Int, from socket: Int32) throws -> Data? {
        var data = Data(count: count)
        var offset = 0
        while offset < count {
            let received = data.withUnsafeMutableBytes { bytes in
                recv(socket, bytes.baseAddress!.advanced(by: offset), count - offset, 0)
            }
            if received == 0 { return nil }
            if received < 0 {
                if errno == EINTR { continue }
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            offset += received
        }
        return data
    }

    private func writeAll(_ data: Data, to socket: Int32) throws {
        var offset = 0
        while offset < data.count {
            let sent = data.withUnsafeBytes { bytes in
                send(socket, bytes.baseAddress!.advanced(by: offset), data.count - offset, MSG_NOSIGNAL)
            }
            if sent == 0 { throw POSIXError(.EPIPE) }
            if sent < 0 {
                if errno == EINTR { continue }
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            offset += sent
        }
    }

    private func transportError(_ message: String) -> ToolExecutionResult {
        ToolExecutionResult(
            text: message,
            isError: true
        )
    }
}

private func configureRelaySocket(_ descriptor: Int32) throws {
    var timeout = timeval(tv_sec: 30, tv_usec: 0)
    let timeoutSize = socklen_t(MemoryLayout<timeval>.size)
    guard withUnsafePointer(to: &timeout, {
        setsockopt(descriptor, SOL_SOCKET, SO_RCVTIMEO, $0, timeoutSize)
    }) == 0,
    withUnsafePointer(to: &timeout, {
        setsockopt(descriptor, SOL_SOCKET, SO_SNDTIMEO, $0, timeoutSize)
    }) == 0
    else {
        throw POSIXError(.init(rawValue: errno) ?? .EIO)
    }
}

private func relayUnixAddress(path: String) throws -> sockaddr_un {
    var address = sockaddr_un()
    address.sun_family = sa_family_t(AF_UNIX)
    let bytes = Array(path.utf8)
    let capacity = MemoryLayout.size(ofValue: address.sun_path)
    guard bytes.count < capacity else {
        throw ComputerUseError.stateUnavailable("The app-agent socket path is too long.")
    }
    withUnsafeMutablePointer(to: &address.sun_path) { pointer in
        pointer.withMemoryRebound(to: CChar.self, capacity: capacity) { buffer in
            for index in bytes.indices { buffer[index] = CChar(bitPattern: bytes[index]) }
            buffer[bytes.count] = 0
        }
    }
    return address
}
