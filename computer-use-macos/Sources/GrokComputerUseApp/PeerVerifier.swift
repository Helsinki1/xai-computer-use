import ComputerUseCore
import Darwin
import Foundation
import Security

struct VerifiedPeer: Sendable {
    let processIdentifier: pid_t
    let userIdentifier: uid_t
}

protocol PeerVerifying: Sendable {
    func verify(socket: Int32) throws -> VerifiedPeer
}

#if DEBUG
/// The test account is the security boundary for local E2E.  This verifier keeps
/// the private-socket UID and bundled-relay path checks, but deliberately does
/// not require an Apple-issued identity (which an ad-hoc build cannot have).
final class LocalE2ERelayPeerVerifier: PeerVerifying, @unchecked Sendable {
    private let trustedRelayURL: URL

    init(appBundleURL: URL = Bundle.main.bundleURL) {
        trustedRelayURL = appBundleURL.resolvingSymlinksInPath().standardizedFileURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("MacOS", isDirectory: true)
            .appendingPathComponent(ComputerUsePaths.relayExecutableName, isDirectory: false)
            .resolvingSymlinksInPath()
            .standardizedFileURL
    }

    func verify(socket: Int32) throws -> VerifiedPeer {
        var peerUID = uid_t.max
        var peerGID = gid_t.max
        guard getpeereid(socket, &peerUID, &peerGID) == 0, peerUID == geteuid() else {
            throw ComputerUseError.permissionDenied("The local E2E relay has an untrusted user identity.")
        }
        var peerPID = pid_t(0)
        var peerPIDSize = socklen_t(MemoryLayout<pid_t>.size)
        guard getsockopt(socket, SOL_LOCAL, LOCAL_PEERPID, &peerPID, &peerPIDSize) == 0, peerPID > 0,
              executableURL(for: peerPID) == trustedRelayURL
        else {
            throw ComputerUseError.permissionDenied("The local E2E peer is not this app's bundled relay.")
        }
        return VerifiedPeer(processIdentifier: peerPID, userIdentifier: peerUID)
    }

    private func executableURL(for processIdentifier: pid_t) -> URL? {
        var buffer = [CChar](repeating: 0, count: 4 * 1024)
        guard proc_pidpath(processIdentifier, &buffer, UInt32(buffer.count)) > 0 else { return nil }
        return URL(fileURLWithPath: String(cString: buffer)).resolvingSymlinksInPath().standardizedFileURL
    }
}
#endif

final class SignedRelayPeerVerifier: PeerVerifying, @unchecked Sendable {
    private let appBundleURL: URL
    private let trustedRelayURL: URL

    init(appBundleURL: URL = Bundle.main.bundleURL) {
        let resolvedBundleURL = appBundleURL.resolvingSymlinksInPath().standardizedFileURL
        self.appBundleURL = resolvedBundleURL
        trustedRelayURL = resolvedBundleURL
            .appendingPathComponent("Contents", isDirectory: true)
            .appendingPathComponent("MacOS", isDirectory: true)
            .appendingPathComponent(ComputerUsePaths.relayExecutableName, isDirectory: false)
            .resolvingSymlinksInPath()
            .standardizedFileURL
    }

    func verify(socket: Int32) throws -> VerifiedPeer {
        let appPathBeforeValidation = try executableURL(for: getpid())
        let appCode = try Self.guestCode(processIdentifier: getpid())
        let appIdentity = try Self.runningSigningIdentity(
            code: appCode,
            executableBasename: appPathBeforeValidation.lastPathComponent
        )
        guard GrokComponentSigningPolicy.isTrustedApp(appIdentity) else {
            throw ComputerUseError.permissionDenied("The running app identity is not trusted.")
        }
        try Self.requireAppleSignature(
            code: appCode,
            identifier: GrokComponentSigningPolicy.appIdentifier,
            teamIdentifier: appIdentity.teamIdentifier
        )
        try verifyBundleSeal(appIdentity: appIdentity)

        var peerUID = uid_t.max
        var peerGID = gid_t.max
        guard getpeereid(socket, &peerUID, &peerGID) == 0, peerUID == geteuid() else {
            throw ComputerUseError.permissionDenied("The app-agent connection has an untrusted user identity.")
        }

        var peerPID = pid_t(0)
        var peerPIDSize = socklen_t(MemoryLayout<pid_t>.size)
        guard getsockopt(socket, SOL_LOCAL, LOCAL_PEERPID, &peerPID, &peerPIDSize) == 0, peerPID > 0 else {
            throw ComputerUseError.permissionDenied("The app-agent connection has no verifiable process identity.")
        }

        let relayCode = try Self.guestCode(auditToken: peerAuditToken(socket: socket))
        let relayPathBeforeValidation = try Self.executableURL(for: relayCode)
        guard relayPathBeforeValidation == trustedRelayURL else {
            throw ComputerUseError.permissionDenied("The app-agent peer is not the bundled relay executable.")
        }
        try Self.requireAppleSignature(
            code: relayCode,
            identifier: GrokComponentSigningPolicy.relayIdentifier,
            teamIdentifier: appIdentity.teamIdentifier
        )
        let relayIdentity = try Self.runningSigningIdentity(
            code: relayCode,
            executableBasename: relayPathBeforeValidation.lastPathComponent
        )
        let relayPathAfterValidation = try Self.executableURL(for: relayCode)
        let appPathAfterValidation = try executableURL(for: getpid())
        guard relayPathAfterValidation == relayPathBeforeValidation,
              appPathAfterValidation == appPathBeforeValidation,
              GrokComponentSigningPolicy.accepts(app: appIdentity, relay: relayIdentity)
        else {
            throw ComputerUseError.permissionDenied("The app-agent peer code signature is not trusted.")
        }
        return VerifiedPeer(processIdentifier: peerPID, userIdentifier: peerUID)
    }

    private func executableURL(for processIdentifier: pid_t) throws -> URL {
        // PROC_PIDPATHINFO_MAXSIZE is a C macro that Swift cannot import.
        // Its macOS definition is 4 * MAXPATHLEN (4 * 1024).
        var buffer = [CChar](repeating: 0, count: 4 * 1024)
        let count = proc_pidpath(processIdentifier, &buffer, UInt32(buffer.count))
        guard count > 0 else {
            throw ComputerUseError.permissionDenied("The app-agent peer executable path is unavailable.")
        }
        return URL(fileURLWithPath: String(cString: buffer))
            .resolvingSymlinksInPath()
            .standardizedFileURL
    }

    private static func runningSigningIdentity(
        code: SecCode,
        executableBasename: String
    ) throws -> ProcessSigningIdentity {
        var staticCode: SecStaticCode?
        var rawInformation: CFDictionary?
        guard SecCodeCopyStaticCode(code, SecCSFlags(rawValue: 0), &staticCode) == errSecSuccess,
              let staticCode,
              SecCodeCopySigningInformation(
            staticCode,
            SecCSFlags(rawValue: kSecCSSigningInformation),
            &rawInformation
        ) == errSecSuccess,
              let information = rawInformation as? [String: Any],
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

    private static func guestCode(processIdentifier: pid_t) throws -> SecCode {
        let attributes = [
            kSecGuestAttributePid as String: NSNumber(value: processIdentifier),
        ] as CFDictionary
        var guestCode: SecCode?
        guard SecCodeCopyGuestWithAttributes(
            nil,
            attributes,
            SecCSFlags(rawValue: 0),
            &guestCode
        ) == errSecSuccess, let guestCode else {
            throw ComputerUseError.permissionDenied("The component process code is unavailable.")
        }
        return guestCode
    }

    private func peerAuditToken(socket: Int32) throws -> audit_token_t {
        var token = audit_token_t()
        var tokenSize = socklen_t(MemoryLayout<audit_token_t>.size)
        guard getsockopt(socket, SOL_LOCAL, LOCAL_PEERTOKEN, &token, &tokenSize) == 0,
              tokenSize == MemoryLayout<audit_token_t>.size
        else {
            throw ComputerUseError.permissionDenied("The app-agent connection has no stable process credential.")
        }
        return token
    }

    private static func guestCode(auditToken: audit_token_t) throws -> SecCode {
        let tokenData = withUnsafeBytes(of: auditToken) { Data($0) }
        let attributes = [
            kSecGuestAttributeAudit as String: tokenData,
        ] as CFDictionary
        var guestCode: SecCode?
        guard SecCodeCopyGuestWithAttributes(
            nil,
            attributes,
            SecCSFlags(rawValue: 0),
            &guestCode
        ) == errSecSuccess, let guestCode else {
            throw ComputerUseError.permissionDenied("The component process code is unavailable.")
        }
        return guestCode
    }

    private static func executableURL(for code: SecCode) throws -> URL {
        var staticCode: SecStaticCode?
        var rawInformation: CFDictionary?
        guard SecCodeCopyStaticCode(code, SecCSFlags(rawValue: 0), &staticCode) == errSecSuccess,
              let staticCode,
              SecCodeCopySigningInformation(
            staticCode,
            SecCSFlags(rawValue: kSecCSSigningInformation),
            &rawInformation
        ) == errSecSuccess,
              let information = rawInformation as? [String: Any],
              let executableURL = information[kSecCodeInfoMainExecutable as String] as? URL
        else {
            throw ComputerUseError.permissionDenied("The component executable path is unavailable.")
        }
        return executableURL.resolvingSymlinksInPath().standardizedFileURL
    }

    private static func requireAppleSignature(
        code: SecCode,
        identifier: String,
        teamIdentifier: String
    ) throws {
        let requirement = try appleRequirement(identifier: identifier, teamIdentifier: teamIdentifier)
        guard SecCodeCheckValidity(
            code,
            SecCSFlags(rawValue: 0),
            requirement
        ) == errSecSuccess else {
            throw ComputerUseError.permissionDenied("The component process code signature is not trusted.")
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

    private func verifyBundleSeal(appIdentity: ProcessSigningIdentity) throws {
        var appCode: SecStaticCode?
        let flags = SecCSFlags(rawValue: 0)
        let requirement = try Self.appleRequirement(
            identifier: GrokComponentSigningPolicy.appIdentifier,
            teamIdentifier: appIdentity.teamIdentifier
        )
        guard SecStaticCodeCreateWithPath(appBundleURL as CFURL, flags, &appCode) == errSecSuccess,
              let appCode,
              SecStaticCodeCheckValidity(appCode, flags, requirement) == errSecSuccess
        else {
            throw ComputerUseError.permissionDenied("The installed app bundle seal is invalid.")
        }
    }
}
