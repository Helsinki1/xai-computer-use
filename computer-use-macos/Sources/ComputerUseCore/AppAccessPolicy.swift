import Foundation

/// Application targets that computer use must never observe or control.
/// Keeping this policy in ComputerUseCore applies it to every desktop driver,
/// including test and future platform implementations.
public enum AppAccessPolicy {
    private static let blockedBundleIdentifiers: Set<String> = [
        "com.apple.systempreferences",
    ]

    public static func isBlocked(bundleIdentifier: String?) -> Bool {
        guard let bundleIdentifier else { return false }
        return blockedBundleIdentifiers.contains(bundleIdentifier.lowercased())
    }

    public static func requireAllowed(bundleIdentifier: String?) throws {
        guard !isBlocked(bundleIdentifier: bundleIdentifier) else {
            throw ComputerUseError.permissionDenied(
                "System Settings is not available to computer use."
            )
        }
    }

    /// Prevent a semantic accessibility action from following a control whose
    /// explicit purpose is to launch System Settings from another application.
    /// Pixel actions remain governed by the model-facing policy because their
    /// target has no semantic label at dispatch time.
    public static func isSystemSettingsLaunchControl(_ element: AccessibilityElementSnapshot) -> Bool {
        let label = (element.label ?? element.value ?? "")
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased()
        return label == "open system settings" || label == "open system preferences"
    }
}
