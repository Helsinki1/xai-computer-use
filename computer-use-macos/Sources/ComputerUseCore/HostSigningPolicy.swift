package struct ProcessSigningIdentity: Sendable, Equatable {
    package let identifier: String
    package let teamIdentifier: String
    package let executableBasename: String

    package init(identifier: String, teamIdentifier: String, executableBasename: String) {
        self.identifier = identifier
        self.teamIdentifier = teamIdentifier
        self.executableBasename = executableBasename
    }
}

package enum GrokComponentSigningPolicy {
    package static let appIdentifier = "com.xai.grok.computer-use"
    package static let appBasename = "GrokComputerUseApp"
    package static let relayIdentifier = "com.xai.grok.computer-use.mcp"
    package static let relayBasename = "grok-computer-use-mcp"

    package static func accepts(app: ProcessSigningIdentity, relay: ProcessSigningIdentity) -> Bool {
        isTrustedApp(app)
            && isTrustedRelay(relay)
            && app.teamIdentifier == relay.teamIdentifier
    }

    package static func isTrustedApp(_ identity: ProcessSigningIdentity) -> Bool {
        identity.identifier == appIdentifier
            && identity.executableBasename == appBasename
            && GrokHostSigningPolicy.isValidTeamIdentifier(identity.teamIdentifier)
    }

    package static func isTrustedRelay(_ identity: ProcessSigningIdentity) -> Bool {
        identity.identifier == relayIdentifier
            && identity.executableBasename == relayBasename
            && GrokHostSigningPolicy.isValidTeamIdentifier(identity.teamIdentifier)
    }
}

package enum GrokHostSigningPolicy {
    private static let exactHostNames: Set<String> = ["grok", "xai-grok-pager"]
    private static let reverseDNSHostIdentifiers: Set<String> = [
        "ai.x.grok",
        "com.xai.grok",
        "com.xai.grok.cli",
    ]
    private static let releasePrefix = "grok-"
    private static let releaseSuffix = "-macos-aarch64"

    package static func accepts(relay: ProcessSigningIdentity, host: ProcessSigningIdentity) -> Bool {
        guard GrokComponentSigningPolicy.isTrustedRelay(relay),
              relay.teamIdentifier == host.teamIdentifier,
              isAllowedHostName(host.executableBasename)
        else {
            return false
        }

        return isAllowedHostName(host.identifier)
            || reverseDNSHostIdentifiers.contains(host.identifier)
    }

    private static func isAllowedHostName(_ value: String) -> Bool {
        exactHostNames.contains(value) || isVersionedReleaseName(value)
    }

    package static func isValidTeamIdentifier(_ value: String) -> Bool {
        (1...64).contains(value.utf8.count)
            && value.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber) })
    }

    private static func isVersionedReleaseName(_ value: String) -> Bool {
        guard value.hasPrefix(releasePrefix), value.hasSuffix(releaseSuffix) else { return false }
        let start = value.index(value.startIndex, offsetBy: releasePrefix.count)
        let end = value.index(value.endIndex, offsetBy: -releaseSuffix.count)
        return start < end && isSemanticVersion(value[start..<end])
    }

    private static func isSemanticVersion(_ version: Substring) -> Bool {
        guard !version.isEmpty, version.utf8.count <= 128 else { return false }

        let buildSplit = version.split(separator: "+", maxSplits: 1, omittingEmptySubsequences: false)
        guard buildSplit.count <= 2,
              buildSplit.count == 1 || validIdentifiers(buildSplit[1], numericLeadingZerosAllowed: true)
        else {
            return false
        }

        let prereleaseSplit = buildSplit[0].split(separator: "-", maxSplits: 1, omittingEmptySubsequences: false)
        guard prereleaseSplit.count <= 2,
              prereleaseSplit.count == 1 || validIdentifiers(prereleaseSplit[1], numericLeadingZerosAllowed: false)
        else {
            return false
        }

        let core = prereleaseSplit[0].split(separator: ".", omittingEmptySubsequences: false)
        return core.count == 3 && core.allSatisfy(validCoreNumber)
    }

    private static func validCoreNumber(_ value: Substring) -> Bool {
        !value.isEmpty
            && value.allSatisfy({ $0.isASCII && $0.isNumber })
            && (value.count == 1 || value.first != "0")
    }

    private static func validIdentifiers(_ value: Substring, numericLeadingZerosAllowed: Bool) -> Bool {
        let identifiers = value.split(separator: ".", omittingEmptySubsequences: false)
        return !identifiers.isEmpty && identifiers.allSatisfy { identifier in
            guard !identifier.isEmpty,
                  identifier.allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "-") })
            else {
                return false
            }
            if !numericLeadingZerosAllowed,
               identifier.allSatisfy({ $0.isASCII && $0.isNumber }),
               identifier.count > 1,
               identifier.first == "0"
            {
                return false
            }
            return true
        }
    }
}
