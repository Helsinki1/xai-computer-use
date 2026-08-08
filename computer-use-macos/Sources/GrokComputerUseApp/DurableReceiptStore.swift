import ComputerUseCore
import CryptoKit
import Darwin
import Foundation
import Security
import SQLite3

final class DurableReceiptStore: ActionReceiptStoring, @unchecked Sendable {
    private static let keychainService = "com.xai.grok.computer-use.receipt-hmac"
    private static let keychainAccount = "receipt-v2"
    private static let databaseName = "receipts-v2.sqlite3"
    private static let databaseSidecarSuffixes = ["", "-wal", "-shm", "-journal"]
    private let lock = NSLock()
    private let authenticationKey: SymmetricKey
    private var database: OpaquePointer?

    init(directory: URL) throws {
        try Self.preparePrivateDirectory(directory)
        let databaseURL = directory.appendingPathComponent(Self.databaseName, isDirectory: false)
        try Self.prepareDatabaseFile(databaseURL)
        try Self.secureExistingDatabaseSidecars(databaseURL: databaseURL)
        authenticationKey = try Self.loadOrCreateAuthenticationKey()
        let flags = SQLITE_OPEN_CREATE | SQLITE_OPEN_READWRITE | SQLITE_OPEN_FULLMUTEX | SQLITE_OPEN_NOFOLLOW
        let openResult = sqlite3_open_v2(databaseURL.path, &database, flags, nil)
        guard openResult == SQLITE_OK, database != nil else {
            if database != nil {
                sqlite3_close_v2(database)
                database = nil
            }
            throw ComputerUseError.internalFailure("Could not open the durable receipt database.")
        }
        do {
            try Self.secureExistingDatabaseFiles(databaseURL: databaseURL)
            sqlite3_busy_timeout(database, 5_000)
            try execute("PRAGMA journal_mode=WAL")
            try execute("PRAGMA synchronous=FULL")
            try execute("PRAGMA fullfsync=ON")
            try execute("PRAGMA checkpoint_fullfsync=ON")
            try execute("PRAGMA secure_delete=ON")
            try execute("PRAGMA wal_autocheckpoint=1")
            try execute("""
                CREATE TABLE IF NOT EXISTS action_receipts (
                    identifier TEXT PRIMARY KEY NOT NULL,
                    tool_name TEXT NOT NULL,
                    snapshot_identifier TEXT NOT NULL,
                    state TEXT NOT NULL,
                    created_at REAL NOT NULL,
                    updated_at REAL NOT NULL,
                    failure_code TEXT,
                    authentication BLOB NOT NULL
                ) STRICT
                """)
            try execute("""
                CREATE INDEX IF NOT EXISTS action_receipts_dispatched
                    ON action_receipts(updated_at)
                 WHERE state = 'dispatched'
                """)
            guard try scalarText("PRAGMA journal_mode")?.lowercased() == "wal",
                  try scalarInt("PRAGMA synchronous") == 2,
                  try scalarInt("PRAGMA fullfsync") == 1
            else {
                throw ComputerUseError.internalFailure("The receipt database could not enable its durability contract.")
            }
            try Self.secureExistingDatabaseFiles(databaseURL: databaseURL)
        } catch {
            sqlite3_close_v2(database)
            database = nil
            throw error
        }
    }

    deinit {
        sqlite3_close_v2(database)
    }

    func load(identifier: String) throws -> ActionReceipt? {
        lock.lock()
        defer { lock.unlock() }
        return try loadUnlocked(identifier: identifier)
    }

    func create(_ receipt: ActionReceipt) throws {
        lock.lock()
        defer { lock.unlock() }
        guard receipt.state == .prepared else {
            throw ComputerUseError.internalFailure("Invalid initial receipt state.")
        }
        try transaction {
            guard try loadUnlocked(identifier: receipt.identifier) == nil else {
                throw ComputerUseError.internalFailure("A receipt with this action identity already exists.")
            }
            try insertUnlocked(receipt)
        }
    }

    func replace(_ receipt: ActionReceipt) throws {
        lock.lock()
        defer { lock.unlock() }
        try transaction {
            guard let previous = try loadUnlocked(identifier: receipt.identifier),
                  previous.toolName == receipt.toolName,
                  previous.snapshotIdentifier == receipt.snapshotIdentifier,
                  allowedTransition(from: previous.state, to: receipt.state)
            else {
                throw ComputerUseError.internalFailure("Invalid receipt state transition.")
            }
            try updateUnlocked(receipt)
        }
    }

    func recoverDispatched(at date: Date) throws {
        lock.lock()
        defer { lock.unlock() }
        try transaction {
            let dispatched = try dispatchedReceiptsUnlocked()
            for receipt in dispatched {
                try updateUnlocked(receipt.transitioned(
                    to: .outcomeUnknown,
                    at: date,
                    failureCode: "agent_restart"
                ))
            }
        }
    }

    private func loadUnlocked(identifier: String) throws -> ActionReceipt? {
        let statement = try prepare("""
            SELECT identifier, tool_name, snapshot_identifier, state, created_at, updated_at,
                   failure_code, authentication
              FROM action_receipts WHERE identifier = ?1
            """)
        defer { sqlite3_finalize(statement) }
        try bindText(identifier, to: statement, at: 1)
        let result = sqlite3_step(statement)
        if result == SQLITE_DONE { return nil }
        guard result == SQLITE_ROW else { throw databaseError() }
        return try decodeAndAuthenticate(statement)
    }

    private func dispatchedReceiptsUnlocked() throws -> [ActionReceipt] {
        let statement = try prepare("""
            SELECT identifier, tool_name, snapshot_identifier, state, created_at, updated_at,
                   failure_code, authentication
              FROM action_receipts WHERE state = 'dispatched'
            """)
        defer { sqlite3_finalize(statement) }
        var receipts: [ActionReceipt] = []
        while true {
            let result = sqlite3_step(statement)
            if result == SQLITE_DONE { return receipts }
            guard result == SQLITE_ROW else { throw databaseError() }
            receipts.append(try decodeAndAuthenticate(statement))
        }
    }

    private func decodeAndAuthenticate(_ statement: OpaquePointer?) throws -> ActionReceipt {
        guard let identifier = columnText(statement, 0),
              let toolName = columnText(statement, 1),
              let snapshotIdentifier = columnText(statement, 2),
              let stateName = columnText(statement, 3),
              let state = ActionReceiptState(rawValue: stateName),
              let authentication = columnData(statement, 7)
        else {
            throw ComputerUseError.internalFailure("The receipt database contains an invalid record.")
        }
        let receipt = ActionReceipt(
            identifier: identifier,
            toolName: toolName,
            snapshotIdentifier: snapshotIdentifier,
            state: state,
            createdAt: Date(timeIntervalSince1970: sqlite3_column_double(statement, 4)),
            updatedAt: Date(timeIntervalSince1970: sqlite3_column_double(statement, 5)),
            failureCode: sqlite3_column_type(statement, 6) == SQLITE_NULL ? nil : columnText(statement, 6)
        )
        guard constantTimeEqual(authentication, authenticate(receipt)) else {
            throw ComputerUseError.internalFailure("Receipt authentication failed.")
        }
        return receipt
    }

    private func insertUnlocked(_ receipt: ActionReceipt) throws {
        let statement = try prepare("""
            INSERT INTO action_receipts
                (identifier, tool_name, snapshot_identifier, state, created_at, updated_at,
                 failure_code, authentication)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            """)
        defer { sqlite3_finalize(statement) }
        try bind(receipt, to: statement)
        guard sqlite3_step(statement) == SQLITE_DONE else { throw databaseError() }
    }

    private func updateUnlocked(_ receipt: ActionReceipt) throws {
        let statement = try prepare("""
            UPDATE action_receipts
               SET state=?4, updated_at=?6, failure_code=?7, authentication=?8
             WHERE identifier=?1 AND tool_name=?2 AND snapshot_identifier=?3 AND created_at=?5
            """)
        defer { sqlite3_finalize(statement) }
        try bind(receipt, to: statement)
        guard sqlite3_step(statement) == SQLITE_DONE, sqlite3_changes(database) == 1 else {
            throw databaseError()
        }
    }

    private func bind(_ receipt: ActionReceipt, to statement: OpaquePointer?) throws {
        try bindText(receipt.identifier, to: statement, at: 1)
        try bindText(receipt.toolName, to: statement, at: 2)
        try bindText(receipt.snapshotIdentifier, to: statement, at: 3)
        try bindText(receipt.state.rawValue, to: statement, at: 4)
        guard sqlite3_bind_double(statement, 5, receipt.createdAt.timeIntervalSince1970) == SQLITE_OK,
              sqlite3_bind_double(statement, 6, receipt.updatedAt.timeIntervalSince1970) == SQLITE_OK
        else { throw databaseError() }
        if let failureCode = receipt.failureCode {
            try bindText(failureCode, to: statement, at: 7)
        } else if sqlite3_bind_null(statement, 7) != SQLITE_OK {
            throw databaseError()
        }
        try bindData(authenticate(receipt), to: statement, at: 8)
    }

    private func authenticate(_ receipt: ActionReceipt) -> Data {
        let payload = ReceiptAuthenticationPayload(receipt)
        let data = try! JSONEncoder.agentWire.encode(payload)
        return Data(HMAC<SHA256>.authenticationCode(for: data, using: authenticationKey))
    }

    private func allowedTransition(from: ActionReceiptState, to: ActionReceiptState) -> Bool {
        switch (from, to) {
        case (.prepared, .dispatched), (.prepared, .rejected),
             (.dispatched, .applied), (.dispatched, .rejected), (.dispatched, .outcomeUnknown):
            true
        default:
            false
        }
    }

    private func transaction(_ body: () throws -> Void) throws {
        try execute("BEGIN IMMEDIATE")
        do {
            try body()
            try execute("COMMIT")
        } catch {
            try? execute("ROLLBACK")
            throw error
        }
    }

    private func execute(_ sql: String) throws {
        var errorMessage: UnsafeMutablePointer<CChar>?
        let result = sqlite3_exec(database, sql, nil, nil, &errorMessage)
        if let errorMessage { sqlite3_free(errorMessage) }
        guard result == SQLITE_OK else { throw databaseError() }
    }

    private func prepare(_ sql: String) throws -> OpaquePointer? {
        var statement: OpaquePointer?
        guard sqlite3_prepare_v2(database, sql, -1, &statement, nil) == SQLITE_OK else {
            throw databaseError()
        }
        return statement
    }

    private func scalarText(_ sql: String) throws -> String? {
        let statement = try prepare(sql)
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_ROW else { throw databaseError() }
        return columnText(statement, 0)
    }

    private func scalarInt(_ sql: String) throws -> Int {
        let statement = try prepare(sql)
        defer { sqlite3_finalize(statement) }
        guard sqlite3_step(statement) == SQLITE_ROW else { throw databaseError() }
        return Int(sqlite3_column_int64(statement, 0))
    }

    private func bindText(_ value: String, to statement: OpaquePointer?, at index: Int32) throws {
        let result = value.withCString {
            sqlite3_bind_text(statement, index, $0, -1, sqliteTransient)
        }
        guard result == SQLITE_OK else { throw databaseError() }
    }

    private func bindData(_ value: Data, to statement: OpaquePointer?, at index: Int32) throws {
        let result = value.withUnsafeBytes {
            sqlite3_bind_blob(statement, index, $0.baseAddress, Int32($0.count), sqliteTransient)
        }
        guard result == SQLITE_OK else { throw databaseError() }
    }

    private func columnText(_ statement: OpaquePointer?, _ index: Int32) -> String? {
        guard let pointer = sqlite3_column_text(statement, index) else { return nil }
        return String(cString: pointer)
    }

    private func columnData(_ statement: OpaquePointer?, _ index: Int32) -> Data? {
        guard let pointer = sqlite3_column_blob(statement, index) else { return nil }
        return Data(bytes: pointer, count: Int(sqlite3_column_bytes(statement, index)))
    }

    private func databaseError() -> ComputerUseError {
        .internalFailure("The durable receipt database operation failed.")
    }

    private static func preparePrivateDirectory(_ directory: URL) throws {
        if mkdir(directory.path, mode_t(S_IRWXU)) != 0, errno != EEXIST {
            throw POSIXError(.init(rawValue: errno) ?? .EIO)
        }
        let descriptor = Darwin.open(directory.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW)
        guard descriptor >= 0 else {
            throw ComputerUseError.permissionDenied("The receipt directory is not a private user-owned directory.")
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
            throw ComputerUseError.permissionDenied("The receipt directory is not a private user-owned directory.")
        }
    }

    private static func secureExistingDatabaseFiles(databaseURL: URL) throws {
        for suffix in databaseSidecarSuffixes {
            try secureExistingDatabaseFile(
                URL(fileURLWithPath: databaseURL.path + suffix, isDirectory: false)
            )
        }
    }

    private static func secureExistingDatabaseSidecars(databaseURL: URL) throws {
        for suffix in databaseSidecarSuffixes.dropFirst() {
            try secureExistingDatabaseFile(
                URL(fileURLWithPath: databaseURL.path + suffix, isDirectory: false)
            )
        }
    }

    private static func prepareDatabaseFile(_ url: URL) throws {
        let descriptor = Darwin.open(
            url.path,
            O_CREAT | O_RDWR | O_NOFOLLOW,
            mode_t(S_IRUSR | S_IWUSR)
        )
        guard descriptor >= 0 else {
            throw ComputerUseError.permissionDenied("The durable receipt database could not be created safely.")
        }
        defer { close(descriptor) }
        try secureDatabaseFileDescriptor(descriptor, at: url)
    }

    private static func secureExistingDatabaseFile(_ url: URL) throws {
        let descriptor = Darwin.open(url.path, O_RDONLY | O_NOFOLLOW)
        guard descriptor >= 0 else {
            if errno == ENOENT { return }
            throw ComputerUseError.permissionDenied("A durable receipt state file could not be opened safely.")
        }
        defer { close(descriptor) }

        try secureDatabaseFileDescriptor(descriptor, at: url)
    }

    private static func secureDatabaseFileDescriptor(_ descriptor: Int32, at url: URL) throws {
        var descriptorStatus = stat()
        var pathStatus = stat()
        guard fstat(descriptor, &descriptorStatus) == 0,
              lstat(url.path, &pathStatus) == 0,
              descriptorStatus.st_dev == pathStatus.st_dev,
              descriptorStatus.st_ino == pathStatus.st_ino,
              descriptorStatus.st_uid == geteuid(),
              (descriptorStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              descriptorStatus.st_nlink == 1,
              fchmod(descriptor, mode_t(S_IRUSR | S_IWUSR)) == 0,
              fcntl(descriptor, F_SETFD, FD_CLOEXEC) == 0
        else {
            throw ComputerUseError.permissionDenied("A durable receipt state file is not private and user-owned.")
        }
    }

    private static func loadOrCreateAuthenticationKey() throws -> SymmetricKey {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        if status == errSecSuccess, let data = item as? Data, data.count == 32 {
            return SymmetricKey(data: data)
        }
        guard status == errSecItemNotFound else {
            throw ComputerUseError.internalFailure("Could not load the receipt authentication key.")
        }
        var bytes = Data(count: 32)
        let randomStatus = bytes.withUnsafeMutableBytes {
            SecRandomCopyBytes(kSecRandomDefault, $0.count, $0.baseAddress!)
        }
        guard randomStatus == errSecSuccess else {
            throw ComputerUseError.internalFailure("Could not create the receipt authentication key.")
        }
        var add = query
        add.removeValue(forKey: kSecReturnData as String)
        add.removeValue(forKey: kSecMatchLimit as String)
        add[kSecValueData as String] = bytes
        add[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(add as CFDictionary, nil)
        if addStatus == errSecDuplicateItem {
            return try loadOrCreateAuthenticationKey()
        }
        guard addStatus == errSecSuccess else {
            throw ComputerUseError.internalFailure("Could not persist the receipt authentication key.")
        }
        return SymmetricKey(data: bytes)
    }
}

private struct ReceiptAuthenticationPayload: Codable {
    let version = 2
    let identifier: String
    let toolName: String
    let snapshotIdentifier: String
    let state: String
    let createdAtBits: UInt64
    let updatedAtBits: UInt64
    let failureCode: String?

    init(_ receipt: ActionReceipt) {
        identifier = receipt.identifier
        toolName = receipt.toolName
        snapshotIdentifier = receipt.snapshotIdentifier
        state = receipt.state.rawValue
        createdAtBits = receipt.createdAt.timeIntervalSince1970.bitPattern
        updatedAtBits = receipt.updatedAt.timeIntervalSince1970.bitPattern
        failureCode = receipt.failureCode
    }
}

private let sqliteTransient = unsafeBitCast(-1, to: sqlite3_destructor_type.self)

private func constantTimeEqual(_ left: Data, _ right: Data) -> Bool {
    guard left.count == right.count else { return false }
    return zip(left, right).reduce(UInt8(0)) { $0 | ($1.0 ^ $1.1) } == 0
}
