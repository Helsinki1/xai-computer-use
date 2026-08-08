import Foundation

public protocol RuntimeClock: Sendable {
    func now() -> Date
}

public struct SystemRuntimeClock: RuntimeClock {
    public init() {}
    public func now() -> Date { Date() }
}

public protocol IdentifierGenerating: Sendable {
    func nextIdentifier() -> String
}

public struct UUIDIdentifierGenerator: IdentifierGenerating {
    public init() {}
    public func nextIdentifier() -> String { UUID().uuidString.lowercased() }
}

public protocol ActionReceiptStoring: Sendable {
    func load(identifier: String) throws -> ActionReceipt?
    func create(_ receipt: ActionReceipt) throws
    func replace(_ receipt: ActionReceipt) throws
    func recoverDispatched(at date: Date) throws
}

public protocol DesktopDriving: Sendable {
    @MainActor func listApps() async throws -> [AppDescriptor]
    @MainActor func capture(bundleIdentifier: String, windowIdentifier: UInt32?) async throws -> CapturedDesktopState
    @MainActor func capture(processIdentifier: Int32, windowIdentifier: UInt32?) async throws -> CapturedDesktopState
    @MainActor func click(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        point: GlobalScreenPoint,
        button: MouseButton,
        count: Int
    ) async throws
    @MainActor func performAccessibilityAction(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        driverToken: String,
        action: String
    ) async throws
    @MainActor func scroll(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        point: GlobalScreenPoint,
        deltaX: Double,
        deltaY: Double
    ) async throws
    @MainActor func drag(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        from: GlobalScreenPoint,
        to: GlobalScreenPoint
    ) async throws
    @MainActor func typeText(app: AppTarget, expectedGeometry: WindowGeometry, text: String) async throws
    @MainActor func pressKey(app: AppTarget, expectedGeometry: WindowGeometry, specification: String) async throws
    @MainActor func setValue(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        driverToken: String,
        value: String
    ) async throws
}

public protocol ToolCalling: Sendable {
    func callTool(name: String, arguments: [String: JSONValue], context: ToolCallContext) async -> ToolExecutionResult
    func actionOutcome(identifier: String) async -> ActionReceipt?
    func clientDisconnected(identifier: String) async
}
