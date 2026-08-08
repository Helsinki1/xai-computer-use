import ComputerUseCore
import Darwin
import Foundation
import Security

enum TrustedHostParentVerifier {
    static func verify() throws {
        let parentPID = getppid()
        guard parentPID > 1 else { throw untrustedParent() }

        let relayCode = try currentCode()
        let relayPath = try executableURL(processIdentifier: getpid())
        let hostPathBeforeValidation = try executableURL(processIdentifier: parentPID)
        let hostCode = try guestCode(processIdentifier: parentPID)

        let relayIdentity = try signingIdentity(
            relayCode,
            executableBasename: relayPath.lastPathComponent
        )
        guard GrokComponentSigningPolicy.isTrustedRelay(relayIdentity) else {
            throw untrustedParent()
        }
        try requireValidSignature(
            relayCode,
            requirement: try appleRequirement(
                identifier: GrokComponentSigningPolicy.relayIdentifier,
                teamIdentifier: relayIdentity.teamIdentifier
            )
        )
        try requireValidSignature(
            hostCode,
            requirement: try appleRequirement(identifier: nil, teamIdentifier: relayIdentity.teamIdentifier)
        )
        let hostIdentity = try signingIdentity(
            hostCode,
            executableBasename: hostPathBeforeValidation.lastPathComponent
        )
        let hostPathAfterValidation = try executableURL(processIdentifier: parentPID)

        guard getppid() == parentPID,
              hostPathBeforeValidation == hostPathAfterValidation,
              GrokHostSigningPolicy.accepts(relay: relayIdentity, host: hostIdentity)
        else {
            throw untrustedParent()
        }
    }

    private static func currentCode() throws -> SecCode {
        var code: SecCode?
        guard SecCodeCopySelf(SecCSFlags(rawValue: 0), &code) == errSecSuccess, let code else {
            throw untrustedParent()
        }
        return code
    }

    private static func guestCode(processIdentifier: pid_t) throws -> SecCode {
        let attributes = [
            kSecGuestAttributePid as String: NSNumber(value: processIdentifier),
        ] as CFDictionary
        var code: SecCode?
        guard SecCodeCopyGuestWithAttributes(
            nil,
            attributes,
            SecCSFlags(rawValue: 0),
            &code
        ) == errSecSuccess, let code else {
            throw untrustedParent()
        }
        return code
    }

    private static func requireValidSignature(_ code: SecCode, requirement: SecRequirement? = nil) throws {
        guard SecCodeCheckValidity(code, SecCSFlags(rawValue: 0), requirement) == errSecSuccess else {
            throw untrustedParent()
        }
    }

    private static func appleRequirement(
        identifier: String?,
        teamIdentifier: String
    ) throws -> SecRequirement {
        guard GrokHostSigningPolicy.isValidTeamIdentifier(teamIdentifier) else {
            throw untrustedParent()
        }
        var expression = "anchor apple generic and certificate leaf[subject.OU] = \"\(teamIdentifier)\""
        if let identifier {
            expression += " and identifier \"\(identifier)\""
        }
        var requirement: SecRequirement?
        guard SecRequirementCreateWithString(
            expression as CFString,
            SecCSFlags(rawValue: 0),
            &requirement
        ) == errSecSuccess, let requirement else {
            throw untrustedParent()
        }
        return requirement
    }

    private static func signingIdentity(
        _ code: SecCode,
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
              let teamIdentifier = information[kSecCodeInfoTeamIdentifier as String] as? String,
              !identifier.isEmpty,
              !teamIdentifier.isEmpty
        else {
            throw untrustedParent()
        }
        return ProcessSigningIdentity(
            identifier: identifier,
            teamIdentifier: teamIdentifier,
            executableBasename: executableBasename
        )
    }

    private static func executableURL(processIdentifier: pid_t) throws -> URL {
        // PROC_PIDPATHINFO_MAXSIZE is a C macro that Swift cannot import.
        // Its macOS definition is 4 * MAXPATHLEN (4 * 1024).
        var buffer = [CChar](repeating: 0, count: 4 * 1024)
        guard proc_pidpath(processIdentifier, &buffer, UInt32(buffer.count)) > 0 else {
            throw untrustedParent()
        }
        return URL(fileURLWithPath: String(cString: buffer))
            .resolvingSymlinksInPath()
            .standardizedFileURL
    }

    private static func untrustedParent() -> ComputerUseError {
        .permissionDenied("The MCP relay parent is not a trusted Grok host.")
    }
}
