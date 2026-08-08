#if DEBUG
import AppKit
import ComputerUseCore
import Foundation

/// Test-only containment for local E2E runs. This is absent from release builds.
@MainActor
final class LocalE2EDesktopDriver: DesktopDriving, @unchecked Sendable {
    private let base: any DesktopDriving
    private let allowedBundleIDs: Set<String>

    init?(base: any DesktopDriving, environment: [String: String] = ProcessInfo.processInfo.environment) {
        guard let identifiers = LocalE2EConfiguration.allowedBundleIdentifiers(environment: environment) else { return nil }
        self.base = base
        allowedBundleIDs = identifiers
    }

    func listApps() async throws -> [AppDescriptor] {
        try await base.listApps().filter { app in
            app.bundleIdentifier.map(allowedBundleIDs.contains) ?? false
        }
    }

    func capture(bundleIdentifier: String, windowIdentifier: UInt32?) async throws -> CapturedDesktopState {
        try requireAllowed(bundleIdentifier)
        try approve("Capture window", detail: bundleIdentifier)
        return try await base.capture(bundleIdentifier: bundleIdentifier, windowIdentifier: windowIdentifier)
    }

    func capture(processIdentifier: Int32, windowIdentifier: UInt32?) async throws -> CapturedDesktopState {
        throw ComputerUseError.permissionDenied("Local E2E requires an allowlisted bundle identifier.")
    }

    func click(app: AppTarget, expectedGeometry: WindowGeometry, point: GlobalScreenPoint, button: MouseButton, count: Int) async throws {
        try approveAction("Click \(button.rawValue) ×\(count)", app: app)
        try await base.click(app: app, expectedGeometry: expectedGeometry, point: point, button: button, count: count)
    }

    func performAccessibilityAction(app: AppTarget, expectedGeometry: WindowGeometry, driverToken: String, action: String) async throws {
        try approveAction("Accessibility action: \(action)", app: app)
        try await base.performAccessibilityAction(app: app, expectedGeometry: expectedGeometry, driverToken: driverToken, action: action)
    }

    func scroll(app: AppTarget, expectedGeometry: WindowGeometry, point: GlobalScreenPoint, deltaX: Double, deltaY: Double) async throws {
        try approveAction("Scroll", app: app)
        try await base.scroll(app: app, expectedGeometry: expectedGeometry, point: point, deltaX: deltaX, deltaY: deltaY)
    }

    func drag(app: AppTarget, expectedGeometry: WindowGeometry, from: GlobalScreenPoint, to: GlobalScreenPoint) async throws {
        try approveAction("Drag", app: app)
        try await base.drag(app: app, expectedGeometry: expectedGeometry, from: from, to: to)
    }

    func typeText(app: AppTarget, expectedGeometry: WindowGeometry, text: String) async throws {
        try approveAction("Type \(text.count) characters", app: app)
        try await base.typeText(app: app, expectedGeometry: expectedGeometry, text: text)
    }

    func pressKey(app: AppTarget, expectedGeometry: WindowGeometry, specification: String) async throws {
        try approveAction("Press key: \(specification)", app: app)
        try await base.pressKey(app: app, expectedGeometry: expectedGeometry, specification: specification)
    }

    func setValue(app: AppTarget, expectedGeometry: WindowGeometry, driverToken: String, value: String) async throws {
        try approveAction("Set value (\(value.count) characters)", app: app)
        try await base.setValue(app: app, expectedGeometry: expectedGeometry, driverToken: driverToken, value: value)
    }

    private func approveAction(_ action: String, app: AppTarget) throws {
        guard let bundleID = app.bundleIdentifier else { throw ComputerUseError.permissionDenied("Target has no bundle identifier.") }
        try requireAllowed(bundleID)
        try approve(action, detail: bundleID)
    }

    private func requireAllowed(_ bundleID: String) throws {
        guard allowedBundleIDs.contains(bundleID) else { throw ComputerUseError.permissionDenied("Local E2E only permits configured fixture apps.") }
    }

    private func approve(_ action: String, detail: String) throws {
        let alert = NSAlert()
        alert.messageText = "Approve Local E2E Computer Use"
        alert.informativeText = "\(action)\nTarget: \(detail)"
        alert.addButton(withTitle: "Approve")
        alert.addButton(withTitle: "Reject")
        guard alert.runModal() == .alertFirstButtonReturn else { throw ComputerUseError.permissionDenied("Local E2E action was rejected by the user.") }
    }
}
#endif
