import Foundation

/// Opt-in configuration used exclusively by debug-only local E2E plumbing.
/// Release targets never use this to relax a trust decision.
public enum LocalE2EConfiguration {
    public static func allowedBundleIdentifiers(
        environment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Set<String>? {
        guard environment["GROK_COMPUTER_USE_LOCAL_E2E"] == "1",
              let rawIdentifiers = environment["GROK_COMPUTER_USE_LOCAL_E2E_ALLOWED_BUNDLE_IDS"]
        else { return nil }

        let identifiers = rawIdentifiers.split(separator: ",").map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines)
        }
        guard !identifiers.isEmpty,
              identifiers.allSatisfy({ (3 ... 255).contains($0.utf8.count) })
        else { return nil }
        return Set(identifiers)
    }
}
