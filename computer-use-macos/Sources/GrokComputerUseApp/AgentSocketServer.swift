import ComputerUseCore
import Darwin
import Dispatch
import Foundation

private let singletonCallout: CFMessagePortCallBack = { _, _, _, _ in nil }

final class AgentSocketServer: @unchecked Sendable {
    private let listener: Int32
    private let singletonLease: AgentSingletonLease
    private let runtime: any ToolCalling
    private let peerVerifier: any PeerVerifying
    private let connectionCapacity = DispatchSemaphore(value: 64)
    private let stateLock = NSLock()
    private var isRunning = false
    private var isStopped = false

    static func acquireSingletonLease(at socketURL: URL) throws -> AgentSingletonLease {
        let serviceName = "com.xai.grok.computer-use.agent-v2.singleton" as CFString
        guard let bootstrapPort = CFMessagePortCreateLocal(
            kCFAllocatorDefault,
            serviceName,
            singletonCallout,
            nil,
            nil
        ) else {
            throw ComputerUseError.stateUnavailable("Another Grok Computer Use app instance is active.")
        }
        do {
            let directory = socketURL.deletingLastPathComponent()
            try preparePrivateDirectory(directory)
            let descriptor = try acquireSingletonLock(
                directory.appendingPathComponent("agent-v2.lock", isDirectory: false)
            )
            return AgentSingletonLease(bootstrapPort: bootstrapPort, descriptor: descriptor)
        } catch {
            CFMessagePortInvalidate(bootstrapPort)
            throw error
        }
    }

    init(
        socketURL: URL,
        runtime: any ToolCalling,
        peerVerifier: any PeerVerifying,
        singletonLease: AgentSingletonLease
    ) throws {
        self.runtime = runtime
        self.peerVerifier = peerVerifier
        self.singletonLease = singletonLease
        listener = try Self.createListener(at: socketURL)
    }

    deinit {
        stop()
    }

    func start() {
        stateLock.lock()
        guard !isStopped, !isRunning else {
            stateLock.unlock()
            return
        }
        isRunning = true
        stateLock.unlock()
        Thread.detachNewThread { [self] in acceptLoop() }
    }

    func stop() {
        stateLock.lock()
        guard !isStopped else {
            stateLock.unlock()
            return
        }
        isStopped = true
        isRunning = false
        stateLock.unlock()
        shutdown(listener, SHUT_RDWR)
        close(listener)
    }

    private func acceptLoop() {
        while running {
            let client = accept(listener, nil, nil)
            guard client >= 0 else {
                if running { continue }
                return
            }
            guard running else {
                shutdown(client, SHUT_RDWR)
                close(client)
                return
            }
            guard connectionCapacity.wait(timeout: .now()) == .success else {
                shutdown(client, SHUT_RDWR)
                close(client)
                continue
            }
            do {
                _ = try peerVerifier.verify(socket: client)
                try configureAgentSocket(client)
                let connection = AgentConnection(
                    descriptor: client,
                    runtime: runtime,
                    singletonLease: singletonLease,
                    onFinish: { [connectionCapacity = self.connectionCapacity] in
                        connectionCapacity.signal()
                    }
                )
                Thread.detachNewThread { connection.run() }
            } catch {
                connectionCapacity.signal()
                shutdown(client, SHUT_RDWR)
                close(client)
            }
        }
    }

    private var running: Bool {
        stateLock.lock()
        defer { stateLock.unlock() }
        return isRunning && !isStopped
    }

    private static func createListener(at socketURL: URL) throws -> Int32 {
        let directory = socketURL.deletingLastPathComponent()
        try preparePrivateDirectory(directory)
        try removeStaleSocket(socketURL)
        let descriptor = socket(AF_UNIX, SOCK_STREAM, 0)
        guard descriptor >= 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        var didBind = false
        do {
            guard fcntl(descriptor, F_SETFD, FD_CLOEXEC) == 0 else {
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            var address = try unixAddress(path: socketURL.path)
            let result = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.bind(descriptor, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
            didBind = true
            guard chmod(socketURL.path, mode_t(S_IRUSR | S_IWUSR)) == 0 else {
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            guard listen(descriptor, 64) == 0 else {
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            return descriptor
        } catch {
            close(descriptor)
            if didBind { unlink(socketURL.path) }
            throw error
        }
    }

    private static func acquireSingletonLock(_ url: URL) throws -> Int32 {
        let descriptor = Darwin.open(
            url.path,
            O_CREAT | O_RDWR | O_NOFOLLOW,
            mode_t(S_IRUSR | S_IWUSR)
        )
        guard descriptor >= 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        do {
            var status = stat()
            var lock = flock()
            lock.l_type = Int16(F_WRLCK)
            lock.l_whence = Int16(SEEK_SET)
            lock.l_start = 0
            lock.l_len = 0
            guard fstat(descriptor, &status) == 0,
                  status.st_uid == geteuid(),
                  (status.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
                  status.st_nlink == 1,
                  fchmod(descriptor, mode_t(S_IRUSR | S_IWUSR)) == 0,
                  fcntl(descriptor, F_SETFD, FD_CLOEXEC) == 0,
                  fcntl(descriptor, F_SETLK, &lock) == 0
            else {
                throw ComputerUseError.stateUnavailable("Another Grok Computer Use app instance is active.")
            }
            return descriptor
        } catch {
            close(descriptor)
            throw error
        }
    }

    private static func preparePrivateDirectory(_ directory: URL) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let descriptor = Darwin.open(directory.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard descriptor >= 0 else {
            throw ComputerUseError.permissionDenied("The app-agent directory is not a private user-owned directory.")
        }
        defer { close(descriptor) }

        var descriptorStatus = stat()
        var pathStatus = stat()
        guard fstat(descriptor, &descriptorStatus) == 0,
              lstat(directory.path, &pathStatus) == 0,
              descriptorStatus.st_dev == pathStatus.st_dev,
              descriptorStatus.st_ino == pathStatus.st_ino,
              descriptorStatus.st_uid == geteuid(),
              (descriptorStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR),
              fchmod(descriptor, mode_t(S_IRWXU)) == 0,
              fcntl(descriptor, F_SETFD, FD_CLOEXEC) == 0
        else {
            throw ComputerUseError.permissionDenied("The app-agent directory is not a private user-owned directory.")
        }
    }

    private static func removeStaleSocket(_ url: URL) throws {
        var originalStatus = stat()
        guard lstat(url.path, &originalStatus) == 0 else {
            if errno == ENOENT { return }
            throw POSIXError(.init(rawValue: errno) ?? .EIO)
        }
        guard (originalStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK),
              originalStatus.st_uid == geteuid()
        else {
            throw ComputerUseError.permissionDenied("Refusing to replace a non-socket or foreign app-agent path.")
        }

        let probe = socket(AF_UNIX, SOCK_STREAM, 0)
        guard probe >= 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
        let connectionResult: Int32
        let connectionError: Int32
        do {
            guard fcntl(probe, F_SETFL, O_NONBLOCK) == 0 else {
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            var address = try unixAddress(path: url.path)
            connectionResult = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    Darwin.connect(probe, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            connectionError = errno
            close(probe)
        } catch {
            close(probe)
            throw error
        }
        guard connectionResult != 0 else {
            throw ComputerUseError.stateUnavailable("Another app-agent listener is already active.")
        }
        guard connectionError == ECONNREFUSED || connectionError == ENOENT else {
            throw ComputerUseError.stateUnavailable("The existing app-agent socket could not be proven stale.")
        }

        var currentStatus = stat()
        guard lstat(url.path, &currentStatus) == 0 else {
            if errno == ENOENT { return }
            throw POSIXError(.init(rawValue: errno) ?? .EIO)
        }
        guard currentStatus.st_dev == originalStatus.st_dev,
              currentStatus.st_ino == originalStatus.st_ino,
              (currentStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFSOCK),
              currentStatus.st_uid == geteuid()
        else {
            throw ComputerUseError.permissionDenied("The app-agent socket changed during stale-socket validation.")
        }
        guard unlink(url.path) == 0 else { throw POSIXError(.init(rawValue: errno) ?? .EIO) }
    }
}

final class AgentSingletonLease: @unchecked Sendable {
    private let bootstrapPort: CFMessagePort
    private let descriptor: Int32

    init(bootstrapPort: CFMessagePort, descriptor: Int32) {
        self.bootstrapPort = bootstrapPort
        self.descriptor = descriptor
    }

    deinit {
        CFMessagePortInvalidate(bootstrapPort)
        close(descriptor)
    }
}

private final class AgentConnection: @unchecked Sendable {
    private let descriptor: Int32
    private let runtime: any ToolCalling
    // Retaining the process lease prevents a replacement agent from starting
    // while an accepted connection is still completing a durable action.
    private let singletonLease: AgentSingletonLease
    private let onFinish: @Sendable () -> Void
    private var boundClientIdentifier: String?
    private var runtimeClientIdentifier: String?
    private var sessionIdentifier: String?
    private var sessionKey: Data?
    private var lastSequence: UInt64 = 0

    init(
        descriptor: Int32,
        runtime: any ToolCalling,
        singletonLease: AgentSingletonLease,
        onFinish: @escaping @Sendable () -> Void
    ) {
        self.descriptor = descriptor
        self.runtime = runtime
        self.singletonLease = singletonLease
        self.onFinish = onFinish
    }

    func run() {
        defer {
            if let clientIdentifier = runtimeClientIdentifier {
                blockingAwait { await self.runtime.clientDisconnected(identifier: clientIdentifier) }
            }
            shutdown(descriptor, SHUT_RDWR)
            close(descriptor)
            onFinish()
        }

        while true {
            do {
                guard let header = try readExactly(count: MemoryLayout<UInt64>.size) else { return }
                let length = try AgentWire.decodeLength(header)
                guard let payload = try readExactly(count: length) else { return }
                let request = try AgentWire.decodeRequest(payload)
                if request.kind == .initialize {
                    guard sessionKey == nil,
                          boundClientIdentifier == nil,
                          let encodedSecret = request.sessionSecret,
                          let key = Data(base64Encoded: encodedSecret),
                          key.count == 32
                    else {
                        throw ComputerUseError.permissionDenied("Invalid app-agent session initialization.")
                    }
                    boundClientIdentifier = request.clientIdentifier
                    let newSessionIdentifier = UUID().uuidString.lowercased()
                    sessionIdentifier = newSessionIdentifier
                    runtimeClientIdentifier = request.clientIdentifier + ":" + newSessionIdentifier
                    sessionKey = key
                    let proof = try AgentSessionAuthentication.initializationProof(
                        key: key,
                        clientIdentifier: request.clientIdentifier,
                        requestIdentifier: request.requestIdentifier,
                        sessionIdentifier: newSessionIdentifier
                    )
                    let response = AgentResponse(
                        requestIdentifier: request.requestIdentifier,
                        pong: true,
                        sessionIdentifier: newSessionIdentifier,
                        authenticationTag: proof
                    )
                    try writeAll(AgentWire.encodeFrame(response))
                    continue
                }
                guard let boundClientIdentifier,
                      boundClientIdentifier == request.clientIdentifier,
                      let sessionIdentifier,
                      request.sessionIdentifier == sessionIdentifier,
                      let key = sessionKey,
                      let sequence = request.sequence,
                      sequence == lastSequence + 1,
                      let authenticationTag = request.authenticationTag,
                      AgentSessionAuthentication.verify(
                        tag: authenticationTag,
                        key: key,
                        domain: "request-v2",
                        payload: try request.authenticationPayload()
                      )
                else {
                    throw ComputerUseError.permissionDenied("Invalid authenticated app-agent request.")
                }
                lastSequence = sequence
                let response = blockingAwait { await self.handle(request) }
                let authenticated = try response.authenticated(
                    sessionIdentifier: sessionIdentifier,
                    sequence: sequence,
                    key: key
                )
                try writeAll(AgentWire.encodeFrame(authenticated))
            } catch {
                return
            }
        }
    }

    private func handle(_ request: AgentRequest) async -> AgentResponse {
        switch request.kind {
        case .initialize:
            return protocolError(request, code: "invalid_request", message: "Session is already initialized.")
        case .ping:
            return AgentResponse(requestIdentifier: request.requestIdentifier, pong: true)
        case .callTool:
            guard let toolName = request.toolName, let arguments = request.arguments else {
                return protocolError(request, code: "invalid_request", message: "Invalid tool request.")
            }
            let result = await runtime.callTool(
                name: toolName,
                arguments: arguments,
                context: ToolCallContext(
                    clientIdentifier: runtimeClientIdentifier ?? request.clientIdentifier,
                    actionIdentifier: request.actionIdentifier
                )
            )
            return AgentResponse(requestIdentifier: request.requestIdentifier, toolResult: result)
        case .actionOutcome:
            guard let receiptIdentifier = request.receiptIdentifier else {
                return protocolError(request, code: "invalid_request", message: "Invalid outcome request.")
            }
            let receipt = await runtime.actionOutcome(identifier: receiptIdentifier)
            return AgentResponse(requestIdentifier: request.requestIdentifier, receipt: receipt)
        case .attestSnapshotDelivery:
            return await lifecycleResponse(request, toolName: "attest_snapshot_delivery")
        case .invalidateSession:
            return await lifecycleResponse(request, toolName: "invalidate_session")
        case .leaseHeartbeat:
            return await lifecycleResponse(request, toolName: "lease_heartbeat")
        case .releaseOperation:
            return await lifecycleResponse(request, toolName: "release_operation")
        }
    }

    private func lifecycleResponse(_ request: AgentRequest, toolName: String) async -> AgentResponse {
        let result = await runtime.callTool(
            name: toolName,
            arguments: request.arguments ?? [:],
            context: ToolCallContext(clientIdentifier: runtimeClientIdentifier ?? request.clientIdentifier)
        )
        return AgentResponse(requestIdentifier: request.requestIdentifier, toolResult: result)
    }

    private func protocolError(_ request: AgentRequest, code: String, message: String) -> AgentResponse {
        AgentResponse(
            requestIdentifier: request.requestIdentifier,
            error: AgentProtocolError(code: code, message: message)
        )
    }

    private func readExactly(count: Int) throws -> Data? {
        var data = Data(count: count)
        var offset = 0
        while offset < count {
            let received = data.withUnsafeMutableBytes { bytes in
                recv(descriptor, bytes.baseAddress!.advanced(by: offset), count - offset, 0)
            }
            if received == 0 { return offset == 0 ? nil : nil }
            if received < 0 {
                if errno == EINTR { continue }
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            offset += received
        }
        return data
    }

    private func writeAll(_ data: Data) throws {
        var offset = 0
        while offset < data.count {
            let sent = data.withUnsafeBytes { bytes in
                send(descriptor, bytes.baseAddress!.advanced(by: offset), data.count - offset, MSG_NOSIGNAL)
            }
            if sent == 0 { throw POSIXError(.EPIPE) }
            if sent < 0 {
                if errno == EINTR { continue }
                throw POSIXError(.init(rawValue: errno) ?? .EIO)
            }
            offset += sent
        }
    }
}

private func configureAgentSocket(_ descriptor: Int32) throws {
    var timeout = timeval(tv_sec: 60, tv_usec: 0)
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

private final class BlockingResultBox<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value?

    func set(_ value: Value) {
        lock.lock()
        self.value = value
        lock.unlock()
    }

    func get() -> Value {
        lock.lock()
        defer { lock.unlock() }
        return value!
    }
}

private func blockingAwait<Value: Sendable>(_ operation: @escaping @Sendable () async -> Value) -> Value {
    let semaphore = DispatchSemaphore(value: 0)
    let box = BlockingResultBox<Value>()
    Task.detached {
        box.set(await operation())
        semaphore.signal()
    }
    semaphore.wait()
    return box.get()
}

private func unixAddress(path: String) throws -> sockaddr_un {
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
