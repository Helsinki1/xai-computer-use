import CoreGraphics
import CryptoKit
import Foundation
import ImageIO
import UniformTypeIdentifiers
import XCTest
@testable import ComputerUseCore

@MainActor
final class RuntimeTests: XCTestCase {
    func testSnapshotIncludesBoundedVisualTargetCandidatesWithoutDriverTokens() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        let runtime = try makeRuntime(driver: driver, receipts: receipts)

        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )

        XCTAssertFalse(state.isError)
        XCTAssertTrue(state.text.contains("target_candidates"))
        XCTAssertTrue(state.text.contains("id=e1 role=AXButton label=\"Go\" action=AXPress bbox_px=(40.00,40.00,80.00,40.00)"))
        XCTAssertFalse(state.text.contains("driver-e1"))
    }

    func testSnapshotRequiresDeliveryAttestation() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        let runtime = try makeRuntime(driver: driver, receipts: receipts)
        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let carrier = try XCTUnwrap(state.protectedCarrier)

        let rejected = await runtime.callTool(
            name: "click",
            arguments: Self.pixelClick(snapshotID: carrier.snapshotID),
            context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "action-before-attestation")
        )
        XCTAssertTrue(rejected.isError)
        XCTAssertEqual(driver.clickCount, 0)

        let attested = await attest(carrier, runtime: runtime, client: "client-a")
        XCTAssertFalse(attested.isError)
    }

    func testPlanClickDescribesSemanticDispatchWithoutSendingInputOrConsumingSnapshot() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        let runtime = try makeRuntime(driver: driver, receipts: receipts)
        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let carrier = try XCTUnwrap(state.protectedCarrier)
        _ = await attest(carrier, runtime: runtime, client: "client-a")

        let plan = await runtime.callTool(
            name: "plan_click",
            arguments: [
                "snapshot_id": .string(carrier.snapshotID),
                "target": .object(["kind": .string("element"), "element_id": .string("e1")]),
            ],
            context: ToolCallContext(clientIdentifier: "client-a")
        )

        XCTAssertFalse(plan.isError)
        XCTAssertTrue(plan.text.contains("accessibility_action=AXPress"))
        XCTAssertTrue(plan.text.contains("attached image marks the resolved click point"))
        let preview = try XCTUnwrap(plan.imagePNG)
        XCTAssertTrue(hasOpaqueBlackPixel(atTopLeft: PNGPixelPoint(x: 80, y: 60), in: preview))
        XCTAssertEqual(
            try XCTUnwrap(plan.protectedCarrier).pngSHA256,
            SHA256.hash(data: preview).map { String(format: "%02x", $0) }.joined()
        )
        let plannedCarrier = try XCTUnwrap(plan.protectedCarrier)
        let plannedAttestation = await attest(plannedCarrier, runtime: runtime, client: "client-a")
        XCTAssertFalse(plannedAttestation.isError)
        XCTAssertEqual(driver.clickCount, 0)

        let click = await runtime.callTool(
            name: "click",
            arguments: [
                "snapshot_id": .string(plannedCarrier.snapshotID),
                "target": .object(["kind": .string("element"), "element_id": .string("e1")]),
            ],
            context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "planned-click")
        )
        XCTAssertFalse(click.isError)
    }

    func testOnlyFirstConcurrentActionReachesAnEffectAndReceiptIsDispatchedFirst() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        let runtime = try makeRuntime(driver: driver, receipts: receipts)
        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let carrier = try XCTUnwrap(state.protectedCarrier)
        _ = await attest(carrier, runtime: runtime, client: "client-a")

        async let first = runtime.callTool(
            name: "click",
            arguments: Self.pixelClick(snapshotID: carrier.snapshotID),
            context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "action-one")
        )
        async let second = runtime.callTool(
            name: "click",
            arguments: Self.pixelClick(snapshotID: carrier.snapshotID),
            context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "action-two")
        )
        let results = await [first, second]

        XCTAssertEqual(driver.clickCount, 1)
        XCTAssertTrue(driver.sawDurableDispatchBeforeEffect)
        XCTAssertEqual(results.filter(\.isError).count, 1)
        XCTAssertEqual(receipts.receipt("action-one")?.state == .applied || receipts.receipt("action-two")?.state == .applied, true)
    }

    func testDispatchFailureBecomesOutcomeUnknownAndIsNeverRetried() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        driver.failClick = true
        let runtime = try makeRuntime(driver: driver, receipts: receipts)
        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let carrier = try XCTUnwrap(state.protectedCarrier)
        _ = await attest(carrier, runtime: runtime, client: "client-a")
        let context = ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "uncertain-action")

        let first = await runtime.callTool(name: "click", arguments: Self.pixelClick(snapshotID: carrier.snapshotID), context: context)
        let second = await runtime.callTool(name: "click", arguments: Self.pixelClick(snapshotID: carrier.snapshotID), context: context)

        XCTAssertTrue(first.isError)
        XCTAssertTrue(second.isError)
        XCTAssertEqual(driver.clickCount, 1)
        XCTAssertEqual(receipts.receipt("uncertain-action")?.state, .outcomeUnknown)
    }

    func testInvalidKeyIsRejectedBeforeDispatchWithoutConsumingSnapshot() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        let runtime = try makeRuntime(driver: driver, receipts: receipts)
        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let carrier = try XCTUnwrap(state.protectedCarrier)
        _ = await attest(carrier, runtime: runtime, client: "client-a")

        let invalid = await runtime.callTool(
            name: "press_key",
            arguments: [
                "snapshot_id": .string(carrier.snapshotID),
                "key": .string("HyperArrow"),
            ],
            context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "invalid-key")
        )

        XCTAssertTrue(invalid.isError)
        XCTAssertTrue(invalid.text.contains("invalid_arguments"))
        XCTAssertNil(receipts.receipt("invalid-key"))
        XCTAssertTrue(driver.pressedKeySpecifications.isEmpty)

        let valid = await runtime.callTool(
            name: "press_key",
            arguments: [
                "snapshot_id": .string(carrier.snapshotID),
                "key": .string("RightArrow"),
            ],
            context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "valid-key")
        )

        XCTAssertFalse(valid.isError)
        XCTAssertEqual(receipts.receipt("valid-key")?.state, .applied)
        XCTAssertEqual(driver.pressedKeySpecifications, ["right"])
    }

    func testDesktopLeaseFencesOtherClients() async throws {
        let receipts = MemoryReceiptStore()
        let driver = FakeDesktopDriver(receipts: receipts)
        let runtime = try makeRuntime(driver: driver, receipts: receipts)
        _ = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let busy = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-b")
        )
        XCTAssertTrue(busy.isError)
        XCTAssertTrue(busy.text.contains("desktop_busy"))
    }

    func testExpiredCaptureCannotPublishAfterAnotherClientTakesTheLease() async throws {
        let receipts = MemoryReceiptStore()
        let clock = MutableClock()
        let captureGate = OneShotGate()
        let driver = FakeDesktopDriver(receipts: receipts)
        driver.captureGates = [captureGate]
        let runtime = try makeRuntime(driver: driver, receipts: receipts, clock: clock)

        let staleCapture = Task {
            await runtime.callTool(
                name: "get_app_state",
                arguments: ["bundle_id": .string("com.example.fixture")],
                context: ToolCallContext(clientIdentifier: "client-a")
            )
        }
        await captureGate.waitUntilEntered()
        clock.advance(by: 31)

        let currentCapture = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-b")
        )
        captureGate.resume()
        let staleResult = await staleCapture.value

        XCTAssertFalse(currentCapture.isError)
        XCTAssertNotNil(currentCapture.protectedCarrier)
        XCTAssertTrue(staleResult.isError)
        XCTAssertNil(staleResult.protectedCarrier)
    }

    func testSupersededCaptureCannotClearTheNewerSameClientLease() async throws {
        let receipts = MemoryReceiptStore()
        let firstGate = OneShotGate()
        let secondGate = OneShotGate()
        let driver = FakeDesktopDriver(receipts: receipts)
        driver.captureGates = [firstGate, secondGate]
        let runtime = try makeRuntime(driver: driver, receipts: receipts)

        let firstCapture = Task {
            await runtime.callTool(
                name: "get_app_state",
                arguments: ["bundle_id": .string("com.example.fixture")],
                context: ToolCallContext(clientIdentifier: "client-a")
            )
        }
        await firstGate.waitUntilEntered()
        let secondCapture = Task {
            await runtime.callTool(
                name: "get_app_state",
                arguments: ["bundle_id": .string("com.example.fixture")],
                context: ToolCallContext(clientIdentifier: "client-a")
            )
        }
        await secondGate.waitUntilEntered()

        firstGate.resume()
        let staleResult = await firstCapture.value
        secondGate.resume()
        let currentResult = await secondCapture.value

        XCTAssertTrue(staleResult.isError)
        XCTAssertNil(staleResult.protectedCarrier)
        XCTAssertFalse(currentResult.isError)
        XCTAssertNotNil(currentResult.protectedCarrier)
    }

    func testDispatchedActionBlocksLeaseTakeoverUntilItsOutcomeIsRecorded() async throws {
        let receipts = MemoryReceiptStore()
        let clock = MutableClock()
        let clickGate = OneShotGate()
        let driver = FakeDesktopDriver(receipts: receipts)
        driver.clickGate = clickGate
        let runtime = try makeRuntime(driver: driver, receipts: receipts, clock: clock)
        let state = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-a")
        )
        let carrier = try XCTUnwrap(state.protectedCarrier)
        _ = await attest(carrier, runtime: runtime, client: "client-a")

        let action = Task {
            await runtime.callTool(
                name: "click",
                arguments: Self.pixelClick(snapshotID: carrier.snapshotID),
                context: ToolCallContext(clientIdentifier: "client-a", actionIdentifier: "suspended-action")
            )
        }
        await clickGate.waitUntilEntered()
        XCTAssertEqual(receipts.receipt("suspended-action")?.state, .dispatched)
        clock.advance(by: 31)

        let takeover = await runtime.callTool(
            name: "get_app_state",
            arguments: ["bundle_id": .string("com.example.fixture")],
            context: ToolCallContext(clientIdentifier: "client-b")
        )
        XCTAssertTrue(takeover.isError)
        XCTAssertTrue(takeover.text.contains("desktop_busy"))

        clickGate.resume()
        let result = await action.value
        XCTAssertFalse(result.isError)
        XCTAssertEqual(driver.clickCount, 1)
        XCTAssertEqual(receipts.receipt("suspended-action")?.state, .applied)
    }

    private func makeRuntime(
        driver: FakeDesktopDriver,
        receipts: MemoryReceiptStore,
        clock: any RuntimeClock = FixedClock()
    ) throws -> ComputerUseRuntime {
        try ComputerUseRuntime(
            driver: driver,
            receipts: receipts,
            clock: clock,
            identifiers: SequenceIdentifierGenerator(),
            leaseDuration: 30
        )
    }

    private func attest(
        _ carrier: ProtectedComputerUseCarrier,
        runtime: ComputerUseRuntime,
        client: String
    ) async -> ToolExecutionResult {
        await runtime.callTool(
            name: "attest_snapshot_delivery",
            arguments: [
                "snapshot_id": .string(carrier.snapshotID),
                "attestation_id": .string(carrier.attestationID),
                "png_sha256": .string(carrier.pngSHA256),
            ],
            context: ToolCallContext(clientIdentifier: client)
        )
    }

    nonisolated private static func pixelClick(snapshotID: String) -> [String: JSONValue] {
        [
            "snapshot_id": .string(snapshotID),
            "target": .object([
                "kind": .string("pixel"),
                "x_px": .integer(10),
                "y_px": .integer(10),
            ]),
        ]
    }
}

@MainActor
private final class FakeDesktopDriver: DesktopDriving {
    let receipts: MemoryReceiptStore
    var clickCount = 0
    var failClick = false
    var sawDurableDispatchBeforeEffect = false
    var pressedKeySpecifications: [String] = []
    var captureGates: [OneShotGate] = []
    var clickGate: OneShotGate?

    init(receipts: MemoryReceiptStore) {
        self.receipts = receipts
    }

    func listApps() async throws -> [AppDescriptor] {
        [AppDescriptor(name: "Fixture", bundleIdentifier: "com.example.fixture", processIdentifier: 42, isActive: true)]
    }

    func capture(bundleIdentifier: String, windowIdentifier: UInt32?) async throws -> CapturedDesktopState {
        await suspendNextCaptureIfNeeded()
        return state
    }

    func capture(processIdentifier: Int32, windowIdentifier: UInt32?) async throws -> CapturedDesktopState {
        await suspendNextCaptureIfNeeded()
        return state
    }

    func click(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        point: GlobalScreenPoint,
        button: MouseButton,
        count: Int
    ) async throws {
        clickCount += 1
        sawDurableDispatchBeforeEffect = receipts.allReceipts.contains { $0.state == .dispatched }
        await clickGate?.suspendOnce()
        try await Task.sleep(for: .milliseconds(10))
        if failClick { throw ComputerUseError.stateUnavailable("synthetic uncertain dispatch") }
    }

    func performAccessibilityAction(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        driverToken: String,
        action: String
    ) async throws {}
    func scroll(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        point: GlobalScreenPoint,
        deltaX: Double,
        deltaY: Double
    ) async throws {}
    func drag(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        from: GlobalScreenPoint,
        to: GlobalScreenPoint
    ) async throws {}
    func typeText(app: AppTarget, expectedGeometry: WindowGeometry, text: String) async throws {}
    func pressKey(app: AppTarget, expectedGeometry: WindowGeometry, specification: String) async throws {
        pressedKeySpecifications.append(specification)
    }
    func setValue(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        driverToken: String,
        value: String
    ) async throws {}

    private func suspendNextCaptureIfNeeded() async {
        guard !captureGates.isEmpty else { return }
        let gate = captureGates.removeFirst()
        await gate.suspendOnce()
    }

    private var state: CapturedDesktopState {
        CapturedDesktopState(
            app: AppTarget(name: "Fixture", bundleIdentifier: "com.example.fixture", processIdentifier: 42),
            windowTitle: "Fixture",
            geometry: WindowGeometry(
                windowIdentifier: 7,
                globalBoundsPoints: GlobalScreenRect(x: 100, y: 200, width: 400, height: 200),
                pngWidthPixels: 800,
                pngHeightPixels: 400
            ),
            screenshotPNG: fixturePNG(width: 800, height: 400),
            screenshotSHA256: String(repeating: "0", count: 64),
            accessibilityTree: "[e1] AXButton label=\"Go\" actions=AXPress",
            elements: [
                AccessibilityElementSnapshot(
                    identifier: "e1",
                    role: "AXButton",
                    label: "Go",
                    value: nil,
                    frame: GlobalScreenRect(x: 120, y: 220, width: 40, height: 20),
                    actions: ["AXPress"],
                    isValueSettable: false,
                    driverToken: "driver-e1"
                ),
            ]
        )
    }
}

private func fixturePNG(width: Int, height: Int) -> Data {
    let colorSpace = CGColorSpace(name: CGColorSpace.sRGB)!
    let context = CGContext(
        data: nil,
        width: width,
        height: height,
        bitsPerComponent: 8,
        bytesPerRow: width * 4,
        space: colorSpace,
        bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
    )!
    context.setFillColor(CGColor(gray: 0.85, alpha: 1))
    context.fill(CGRect(x: 0, y: 0, width: width, height: height))
    let image = context.makeImage()!
    let data = NSMutableData()
    let destination = CGImageDestinationCreateWithData(data, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(destination, image, nil)
    XCTAssertTrue(CGImageDestinationFinalize(destination))
    return data as Data
}

private func hasOpaqueBlackPixel(atTopLeft point: PNGPixelPoint, in png: Data) -> Bool {
    guard let source = CGImageSourceCreateWithData(png as CFData, nil),
          let image = CGImageSourceCreateImageAtIndex(source, 0, nil),
          let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
          let context = CGContext(
            data: nil,
            width: image.width,
            height: image.height,
            bitsPerComponent: 8,
            bytesPerRow: image.width * 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
          ),
          let data = context.data
    else {
        return false
    }
    context.draw(image, in: CGRect(x: 0, y: 0, width: image.width, height: image.height))
    let x = Int(point.x)
    let y = Int(point.y)
    guard x >= 0, x < image.width, y >= 0, y < image.height else { return false }

    // CGBitmapContext has a bottom-left origin; convert the screenshot's
    // top-left PNG coordinate before inspecting the known RGBA8 buffer.
    let offset = (image.height - 1 - y) * image.width * 4 + x * 4
    let bytes = data.assumingMemoryBound(to: UInt8.self)
    return bytes[offset] == 0 && bytes[offset + 1] == 0 && bytes[offset + 2] == 0 && bytes[offset + 3] == 255
}

private final class MemoryReceiptStore: ActionReceiptStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var values: [String: ActionReceipt] = [:]

    var allReceipts: [ActionReceipt] {
        lock.withLock { Array(values.values) }
    }

    func receipt(_ identifier: String) -> ActionReceipt? {
        lock.withLock { values[identifier] }
    }

    func load(identifier: String) throws -> ActionReceipt? { receipt(identifier) }

    func create(_ receipt: ActionReceipt) throws {
        lock.withLock { values[receipt.identifier] = receipt }
    }

    func replace(_ receipt: ActionReceipt) throws {
        lock.withLock { values[receipt.identifier] = receipt }
    }

    func recoverDispatched(at date: Date) throws {
        lock.withLock {
            let dispatched = values.filter { $0.value.state == .dispatched }
            for (identifier, receipt) in dispatched {
                values[identifier] = receipt.transitioned(to: .outcomeUnknown, at: date)
            }
        }
    }
}

private struct FixedClock: RuntimeClock {
    func now() -> Date { Date(timeIntervalSince1970: 1_800_000_000) }
}

private final class MutableClock: RuntimeClock, @unchecked Sendable {
    private let lock = NSLock()
    private var date = Date(timeIntervalSince1970: 1_800_000_000)

    func now() -> Date {
        lock.withLock { date }
    }

    func advance(by interval: TimeInterval) {
        lock.withLock { date = date.addingTimeInterval(interval) }
    }
}

@MainActor
private final class OneShotGate {
    private var didSuspend = false
    private var didEnter = false
    private var suspension: CheckedContinuation<Void, Never>?
    private var entryWaiters: [CheckedContinuation<Void, Never>] = []

    func suspendOnce() async {
        guard !didSuspend else { return }
        didSuspend = true
        didEnter = true
        let waiters = entryWaiters
        entryWaiters.removeAll()
        waiters.forEach { $0.resume() }
        await withCheckedContinuation { suspension = $0 }
    }

    func waitUntilEntered() async {
        guard !didEnter else { return }
        await withCheckedContinuation { entryWaiters.append($0) }
    }

    func resume() {
        suspension?.resume()
        suspension = nil
    }
}

private final class SequenceIdentifierGenerator: IdentifierGenerating, @unchecked Sendable {
    private let lock = NSLock()
    private var counter: UInt64 = 1

    func nextIdentifier() -> String {
        lock.withLock {
            defer { counter += 1 }
            return String(format: "00000000-0000-4000-8000-%012llu", counter)
        }
    }
}
