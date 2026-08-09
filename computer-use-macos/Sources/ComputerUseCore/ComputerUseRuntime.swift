import Foundation

public actor ComputerUseRuntime: ToolCalling {
    private struct Lease {
        let ownerIdentifier: String
        let fence: UInt64
        var expiresAt: Date
    }

    private struct SnapshotRecord {
        let envelope: SnapshotEnvelope
        let ownerIdentifier: String
        let fence: UInt64
        var consumed: Bool
        var deliveryAttested: Bool
    }

    private struct InFlightOperation {
        let ownerIdentifier: String
        let fence: UInt64
        let actionIdentifier: String
    }

    private let driver: any DesktopDriving
    private let receipts: any ActionReceiptStoring
    private let clock: any RuntimeClock
    private let identifiers: any IdentifierGenerating
    private let leaseDuration: TimeInterval
    private var lease: Lease?
    private var nextFence: UInt64 = 1
    private var snapshots: [String: SnapshotRecord] = [:]
    private var inFlightOperation: InFlightOperation?

    public init(
        driver: any DesktopDriving,
        receipts: any ActionReceiptStoring,
        clock: any RuntimeClock = SystemRuntimeClock(),
        identifiers: any IdentifierGenerating = UUIDIdentifierGenerator(),
        leaseDuration: TimeInterval = 120
    ) throws {
        guard leaseDuration.isFinite, leaseDuration > 0 else {
            throw ComputerUseError.invalidArguments("The desktop lease duration must be finite and positive.")
        }
        self.driver = driver
        self.receipts = receipts
        self.clock = clock
        self.identifiers = identifiers
        self.leaseDuration = leaseDuration
        try receipts.recoverDispatched(at: clock.now())
    }

    public func callTool(
        name: String,
        arguments: [String: JSONValue],
        context: ToolCallContext
    ) async -> ToolExecutionResult {
        do {
            switch name {
            case "list_apps":
                return try await listApps(arguments: arguments)
            case "get_app_state":
                return try await getAppState(arguments: arguments, clientIdentifier: context.clientIdentifier)
            case "click":
                return try await click(arguments: arguments, context: context)
            case "perform_secondary_action":
                return try await performSecondaryAction(arguments: arguments, context: context)
            case "scroll":
                return try await scroll(arguments: arguments, context: context)
            case "drag":
                return try await drag(arguments: arguments, context: context)
            case "type_text":
                return try await typeText(arguments: arguments, context: context)
            case "press_key":
                return try await pressKey(arguments: arguments, context: context)
            case "set_value":
                return try await setValue(arguments: arguments, context: context)
            case "attest_snapshot_delivery":
                return try attestSnapshotDelivery(arguments: arguments, context: context)
            case "invalidate_session":
                return try invalidateSession(arguments: arguments, context: context)
            case "lease_heartbeat":
                return try leaseHeartbeat(arguments: arguments, context: context)
            case "release_operation":
                return try releaseOperation(arguments: arguments, context: context)
            default:
                throw ComputerUseError.invalidArguments("Unknown tool: \(name)")
            }
        } catch let error as ComputerUseError {
            return errorResult(error)
        } catch {
            return errorResult(.internalFailure("The computer-use operation failed."))
        }
    }

    public func actionOutcome(identifier: String) async -> ActionReceipt? {
        try? receipts.load(identifier: identifier)
    }

    public func clientDisconnected(identifier: String) async {
        if lease?.ownerIdentifier == identifier {
            lease = nil
        }
        snapshots = snapshots.filter { $0.value.ownerIdentifier != identifier }
    }

    private func listApps(arguments: [String: JSONValue]) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        try reader.finish()
        let apps = try await driver.listApps()
        var text = "running_apps=\(apps.count)\n"
        for app in apps {
            let windowIDs = app.windowIdentifiers.map(String.init).joined(separator: ",")
            let line = "bundle_id=\(app.bundleIdentifier ?? "<none>") pid=\(app.processIdentifier) active=\(app.isActive) window_ids=[\(windowIDs)] name=\(app.name)\n"
            if text.utf8.count + line.utf8.count > 16 * 1_024 { break }
            text += line
        }
        return ToolExecutionResult(text: text)
    }

    private func getAppState(arguments: [String: JSONValue], clientIdentifier: String) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let bundleIdentifier = try reader.requiredString("bundle_id")
        let requestedWindow = try reader.optionalInteger("window_id")
        try reader.finish()
        guard (3...255).contains(bundleIdentifier.utf8.count),
              requestedWindow == nil || (1...Int(UInt32.max)).contains(requestedWindow!)
        else {
            throw ComputerUseError.invalidArguments("bundle_id or window_id is outside the v2 contract.")
        }

        let activeLease = try acquireLease(for: clientIdentifier)
        do {
            let captured = try await driver.capture(
                bundleIdentifier: bundleIdentifier,
                windowIdentifier: requestedWindow.map(UInt32.init)
            )
            let refreshedLease = try renewLease(
                ownerIdentifier: clientIdentifier,
                fence: activeLease.fence,
                requireUnexpired: true
            )
            let envelope = storeSnapshot(captured, ownerIdentifier: clientIdentifier, lease: refreshedLease)
            return try snapshotResult(envelope, receiptIdentifier: nil)
        } catch {
            if lease?.ownerIdentifier == clientIdentifier,
               lease?.fence == activeLease.fence,
               snapshots.values.allSatisfy({ $0.ownerIdentifier != clientIdentifier })
            {
                lease = nil
            }
            throw error
        }
    }

    private func click(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        var targetReader = ArgumentReader(try reader.requiredObject("target"))
        let kind = try targetReader.requiredString("kind")
        let buttonName = try reader.optionalString("button") ?? MouseButton.left.rawValue
        let count = try reader.optionalInteger("count", default: 1)
        try reader.finish()

        guard let button = MouseButton(rawValue: buttonName), button != .middle, (1...2).contains(count) else {
            throw ComputerUseError.invalidArguments("button or count is invalid.")
        }
        let captured = try validatedSnapshot(identifier: snapshotID, ownerIdentifier: context.clientIdentifier).envelope.captured
        let driver = self.driver
        if kind == "element" {
            let elementID = try targetReader.requiredString("element_id")
            try targetReader.finish()
            guard (1...128).contains(elementID.utf8.count) else {
                throw ComputerUseError.invalidArguments("element_id is outside the v2 contract.")
            }
            let element = try element(identifier: elementID, in: captured)
            if button == .left, count == 1,
               let primary = element.actions.first(where: { $0.caseInsensitiveCompare("AXPress") == .orderedSame })
            {
                return try await dispatchAction(toolName: "click", snapshotID: snapshotID, context: context) { _ in
                    try await driver.performAccessibilityAction(
                        app: captured.app,
                        expectedGeometry: captured.geometry,
                        driverToken: element.driverToken,
                        action: primary
                    )
                }
            }
            guard let point = element.frame?.center else {
                throw ComputerUseError.invalidArguments("The selected element has no actionable frame.")
            }
            return try await dispatchAction(toolName: "click", snapshotID: snapshotID, context: context) { _ in
                try await driver.click(
                    app: captured.app,
                    expectedGeometry: captured.geometry,
                    point: point,
                    button: button,
                    count: count
                )
            }
        } else if kind == "pixel" {
            let pixel = PNGPixelPoint(
                x: try targetReader.requiredNumber("x_px"),
                y: try targetReader.requiredNumber("y_px")
            )
            try targetReader.finish()
            let point = try CoordinateMapper.globalPoint(for: pixel, in: captured.geometry)
            return try await dispatchAction(toolName: "click", snapshotID: snapshotID, context: context) { _ in
                try await driver.click(
                    app: captured.app,
                    expectedGeometry: captured.geometry,
                    point: point,
                    button: button,
                    count: count
                )
            }
        }
        throw ComputerUseError.invalidArguments("target.kind must be element or pixel.")
    }

    private func performSecondaryAction(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let elementID = try reader.requiredString("element_id")
        let requestedAction = try reader.requiredString("action_id")
        try reader.finish()
        guard (1...128).contains(elementID.utf8.count), (1...128).contains(requestedAction.utf8.count) else {
            throw ComputerUseError.invalidArguments("element_id or action_id is outside the v2 contract.")
        }
        let captured = try validatedSnapshot(identifier: snapshotID, ownerIdentifier: context.clientIdentifier).envelope.captured
        let element = try element(identifier: elementID, in: captured)
        guard let action = element.actions.first(where: { $0.caseInsensitiveCompare(requestedAction) == .orderedSame }),
              action.caseInsensitiveCompare("AXPress") != .orderedSame
        else {
            throw ComputerUseError.invalidArguments("The requested secondary action was not advertised by the snapshot.")
        }
        let driver = self.driver
        return try await dispatchAction(toolName: "perform_secondary_action", snapshotID: snapshotID, context: context) { _ in
            try await driver.performAccessibilityAction(
                app: captured.app,
                expectedGeometry: captured.geometry,
                driverToken: element.driverToken,
                action: action
            )
        }
    }

    private func scroll(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let elementID = try reader.requiredString("element_id")
        let direction = try reader.requiredString("direction")
        let pages = try reader.optionalNumber("pages", default: 1)
        try reader.finish()
        guard pages > 0, pages <= 10, pages.isFinite, ["up", "down", "left", "right"].contains(direction) else {
            throw ComputerUseError.invalidArguments("direction or pages is invalid.")
        }
        let captured = try validatedSnapshot(identifier: snapshotID, ownerIdentifier: context.clientIdentifier).envelope.captured
        let element = try element(identifier: elementID, in: captured)
        guard let point = element.frame?.center else {
            throw ComputerUseError.invalidArguments("The selected element has no scroll point.")
        }
        let magnitude = 12 * pages
        let delta: (Double, Double) = switch direction {
        case "up": (0, magnitude)
        case "down": (0, -magnitude)
        case "left": (magnitude, 0)
        default: (-magnitude, 0)
        }
        let driver = self.driver
        return try await dispatchAction(toolName: "scroll", snapshotID: snapshotID, context: context) { _ in
            try await driver.scroll(
                app: captured.app,
                expectedGeometry: captured.geometry,
                point: point,
                deltaX: delta.0,
                deltaY: delta.1
            )
        }
    }

    private func drag(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let from = try pixelPoint(from: reader.requiredObject("from"))
        let to = try pixelPoint(from: reader.requiredObject("to"))
        try reader.finish()
        let captured = try validatedSnapshot(identifier: snapshotID, ownerIdentifier: context.clientIdentifier).envelope.captured
        let globalFrom = try CoordinateMapper.globalPoint(for: from, in: captured.geometry)
        let globalTo = try CoordinateMapper.globalPoint(for: to, in: captured.geometry)
        let driver = self.driver
        return try await dispatchAction(toolName: "drag", snapshotID: snapshotID, context: context) { _ in
            try await driver.drag(
                app: captured.app,
                expectedGeometry: captured.geometry,
                from: globalFrom,
                to: globalTo
            )
        }
    }

    private func typeText(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let text = try reader.requiredString("text", allowEmpty: true)
        try reader.finish()
        guard text.count <= 32_768 else { throw ComputerUseError.invalidArguments("text exceeds the v2 limit.") }
        let driver = self.driver
        return try await dispatchAction(toolName: "type_text", snapshotID: snapshotID, context: context) { record in
            let captured = record.envelope.captured
            try await driver.typeText(app: captured.app, expectedGeometry: captured.geometry, text: text)
        }
    }

    private func pressKey(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let key = try reader.requiredString("key")
        let modifiers = try reader.optionalStringArray("modifiers")
        try reader.finish()
        let allowed = Set(["command", "control", "option", "shift", "fn"])
        guard modifiers.count == Set(modifiers).count, Set(modifiers).isSubset(of: allowed) else {
            throw ComputerUseError.invalidArguments("modifiers is invalid.")
        }
        let specification = try KeyboardShortcut.canonicalSpecification(key: key, modifiers: modifiers)
        let driver = self.driver
        return try await dispatchAction(toolName: "press_key", snapshotID: snapshotID, context: context) { record in
            let captured = record.envelope.captured
            try await driver.pressKey(
                app: captured.app,
                expectedGeometry: captured.geometry,
                specification: specification
            )
        }
    }

    private func setValue(arguments: [String: JSONValue], context: ToolCallContext) async throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let elementID = try reader.requiredString("element_id")
        let value = try reader.requiredString("value", allowEmpty: true)
        try reader.finish()
        guard value.count <= 32_768 else { throw ComputerUseError.invalidArguments("value exceeds the v2 limit.") }
        let captured = try validatedSnapshot(identifier: snapshotID, ownerIdentifier: context.clientIdentifier).envelope.captured
        let element = try element(identifier: elementID, in: captured)
        guard element.isValueSettable else {
            throw ComputerUseError.invalidArguments("The selected element was not settable in the snapshot.")
        }
        let driver = self.driver
        return try await dispatchAction(toolName: "set_value", snapshotID: snapshotID, context: context) { _ in
            try await driver.setValue(
                app: captured.app,
                expectedGeometry: captured.geometry,
                driverToken: element.driverToken,
                value: value
            )
        }
    }

    private func attestSnapshotDelivery(arguments: [String: JSONValue], context: ToolCallContext) throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        let attestationID = try reader.requiredString("attestation_id")
        let pngSHA256 = try reader.requiredString("png_sha256")
        try reader.finish()
        guard var record = snapshots[snapshotID],
              record.ownerIdentifier == context.clientIdentifier,
              record.envelope.deliveryAttestationIdentifier == attestationID,
              record.envelope.captured.screenshotSHA256 == pngSHA256,
              let current = lease,
              current.ownerIdentifier == context.clientIdentifier,
              current.fence == record.fence,
              current.expiresAt > clock.now(),
              !record.consumed
        else {
            throw ComputerUseError.invalidSnapshot
        }
        record.deliveryAttested = true
        snapshots[snapshotID] = record
        return ToolExecutionResult(
            text: "Snapshot delivery attested."
        )
    }

    private func invalidateSession(arguments: [String: JSONValue], context: ToolCallContext) throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        try reader.finish()
        snapshots = snapshots.filter { $0.value.ownerIdentifier != context.clientIdentifier }
        if lease?.ownerIdentifier == context.clientIdentifier { lease = nil }
        return ToolExecutionResult(text: "Computer-use session invalidated.")
    }

    private func leaseHeartbeat(arguments: [String: JSONValue], context: ToolCallContext) throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        try reader.finish()
        guard let record = snapshots[snapshotID],
              record.ownerIdentifier == context.clientIdentifier,
              let current = lease,
              current.ownerIdentifier == context.clientIdentifier,
              current.fence == record.fence,
              current.expiresAt > clock.now()
        else {
            throw ComputerUseError.invalidSnapshot
        }
        try renewLease(
            ownerIdentifier: context.clientIdentifier,
            fence: record.fence,
            requireUnexpired: true
        )
        return ToolExecutionResult(text: "Desktop lease renewed.")
    }

    private func releaseOperation(arguments: [String: JSONValue], context: ToolCallContext) throws -> ToolExecutionResult {
        var reader = ArgumentReader(arguments)
        let snapshotID = try reader.requiredString("snapshot_id")
        try reader.finish()
        guard let record = snapshots[snapshotID],
              record.ownerIdentifier == context.clientIdentifier,
              let current = lease,
              current.ownerIdentifier == context.clientIdentifier,
              current.fence == record.fence,
              inFlightOperation == nil
        else {
            throw ComputerUseError.invalidSnapshot
        }
        snapshots.removeValue(forKey: snapshotID)
        if lease?.ownerIdentifier == context.clientIdentifier { lease = nil }
        return ToolExecutionResult(text: "Desktop operation released.")
    }

    private func dispatchAction(
        toolName: String,
        snapshotID: String,
        context: ToolCallContext,
        operation: @escaping @Sendable (SnapshotRecord) async throws -> Void
    ) async throws -> ToolExecutionResult {
        guard let actionID = context.actionIdentifier,
              !actionID.isEmpty,
              actionID.utf8.count <= 256,
              !actionID.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) })
        else {
            throw ComputerUseError.internalFailure("The relay did not supply a valid action identifier.")
        }

        if let existing = try receipts.load(identifier: actionID) {
            guard existing.toolName == toolName, existing.snapshotIdentifier == snapshotID else {
                throw ComputerUseError.invalidArguments("The action identifier conflicts with an existing receipt.")
            }
            switch existing.state {
            case .applied:
                return recoveredOutcomeResult(existing)
            case .rejected:
                return receiptErrorResult(existing, message: "The action was rejected before dispatch.")
            case .dispatched, .outcomeUnknown:
                throw ComputerUseError.actionOutcomeUnknown("The action may have been applied and will not be retried. Call get_app_state to observe the desktop.")
            case .prepared:
                break
            }
        }

        let record = try validatedSnapshot(identifier: snapshotID, ownerIdentifier: context.clientIdentifier)
        let now = clock.now()
        if try receipts.load(identifier: actionID) == nil {
            try receipts.create(ActionReceipt(
                identifier: actionID,
                toolName: toolName,
                snapshotIdentifier: snapshotID,
                state: .prepared,
                createdAt: now,
                updatedAt: now
            ))
        }

        let prepared = try requiredReceipt(actionID)
        do {
            try receipts.replace(prepared.transitioned(to: .dispatched, at: clock.now()))
        } catch {
            throw ComputerUseError.internalFailure("The action was not sent because its durable dispatch record could not be written.")
        }

        snapshots.removeValue(forKey: snapshotID)
        inFlightOperation = InFlightOperation(
            ownerIdentifier: context.clientIdentifier,
            fence: record.fence,
            actionIdentifier: actionID
        )
        defer {
            if inFlightOperation?.actionIdentifier == actionID {
                inFlightOperation = nil
            }
        }
        _ = try renewLease(
            ownerIdentifier: context.clientIdentifier,
            fence: record.fence,
            reservedBy: actionID
        )

        do {
            try await operation(record)
        } catch {
            let dispatched = try? requiredReceipt(actionID)
            if let dispatched {
                try? receipts.replace(dispatched.transitioned(to: .outcomeUnknown, at: clock.now(), failureCode: "dispatch_error"))
            }
            throw ComputerUseError.actionOutcomeUnknown("The input dispatch did not produce a certain outcome and will not be retried. Call get_app_state to observe the desktop.")
        }

        let leaseRemainedReserved: Bool
        do {
            _ = try renewLease(
                ownerIdentifier: context.clientIdentifier,
                fence: record.fence,
                reservedBy: actionID
            )
            leaseRemainedReserved = true
        } catch {
            leaseRemainedReserved = false
        }

        let dispatched = try requiredReceipt(actionID)
        do {
            try receipts.replace(dispatched.transitioned(to: .applied, at: clock.now()))
        } catch {
            throw ComputerUseError.actionOutcomeUnknown("The input was sent but its applied receipt could not be made durable. It will not be retried.")
        }

        guard leaseRemainedReserved else {
            return ToolExecutionResult(
                text: "Action applied, but the desktop lease changed before the follow-up screenshot. Call get_app_state.",
                isError: false
            )
        }

        do {
            let captured = try await driver.capture(
                processIdentifier: record.envelope.captured.app.processIdentifier,
                windowIdentifier: record.envelope.captured.geometry.windowIdentifier
            )
            let currentLease = try renewLease(
                ownerIdentifier: context.clientIdentifier,
                fence: record.fence,
                reservedBy: actionID
            )
            let next = storeSnapshot(captured, ownerIdentifier: context.clientIdentifier, lease: currentLease)
            return try snapshotResult(next, receiptIdentifier: actionID)
        } catch {
            return ToolExecutionResult(
                text: "Action applied, but the follow-up screenshot failed. Call get_app_state before the next action.",
                isError: false
            )
        }
    }

    private func acquireLease(for ownerIdentifier: String) throws -> Lease {
        let now = clock.now()
        if inFlightOperation != nil {
            throw ComputerUseError.desktopBusy(retryAfterMilliseconds: 250)
        }
        if let current = lease, current.expiresAt > now {
            guard current.ownerIdentifier == ownerIdentifier else {
                let retry = max(1, Int(current.expiresAt.timeIntervalSince(now) * 1_000))
                throw ComputerUseError.desktopBusy(retryAfterMilliseconds: retry)
            }
            snapshots.removeAll(keepingCapacity: true)
            return try issueLease(ownerIdentifier: ownerIdentifier, at: now)
        }

        snapshots.removeAll(keepingCapacity: true)
        return try issueLease(ownerIdentifier: ownerIdentifier, at: now)
    }

    private func issueLease(ownerIdentifier: String, at date: Date) throws -> Lease {
        guard nextFence < UInt64.max else {
            throw ComputerUseError.internalFailure("The desktop lease fence space is exhausted.")
        }
        let acquired = Lease(
            ownerIdentifier: ownerIdentifier,
            fence: nextFence,
            expiresAt: date.addingTimeInterval(leaseDuration)
        )
        nextFence += 1
        lease = acquired
        return acquired
    }

    @discardableResult
    private func renewLease(
        ownerIdentifier: String,
        fence: UInt64,
        requireUnexpired: Bool = false,
        reservedBy actionIdentifier: String? = nil
    ) throws -> Lease {
        guard var current = lease,
              current.ownerIdentifier == ownerIdentifier,
              current.fence == fence,
              !requireUnexpired || current.expiresAt > clock.now()
        else {
            throw ComputerUseError.stateUnavailable("The desktop lease changed while the operation was in progress.")
        }
        if let actionIdentifier {
            guard let operation = inFlightOperation,
                  operation.ownerIdentifier == ownerIdentifier,
                  operation.fence == fence,
                  operation.actionIdentifier == actionIdentifier
            else {
                throw ComputerUseError.stateUnavailable("The desktop operation reservation was lost.")
            }
        }
        current.expiresAt = clock.now().addingTimeInterval(leaseDuration)
        lease = current
        return current
    }

    private func storeSnapshot(_ captured: CapturedDesktopState, ownerIdentifier: String, lease: Lease) -> SnapshotEnvelope {
        snapshots = snapshots.filter { _, value in value.ownerIdentifier != ownerIdentifier }
        let envelope = SnapshotEnvelope(
            snapshotIdentifier: identifiers.nextIdentifier(),
            deliveryAttestationIdentifier: identifiers.nextIdentifier(),
            captured: captured,
            leaseExpiresAt: lease.expiresAt
        )
        snapshots[envelope.snapshotIdentifier] = SnapshotRecord(
            envelope: envelope,
            ownerIdentifier: ownerIdentifier,
            fence: lease.fence,
            consumed: false,
            deliveryAttested: false
        )
        return envelope
    }

    private func validatedSnapshot(identifier: String, ownerIdentifier: String) throws -> SnapshotRecord {
        guard let record = snapshots[identifier], record.ownerIdentifier == ownerIdentifier else {
            throw ComputerUseError.invalidSnapshot
        }
        guard !record.consumed else { throw ComputerUseError.snapshotConsumed }
        guard record.deliveryAttested else {
            throw ComputerUseError.invalidSnapshot
        }
        guard let current = lease,
              current.ownerIdentifier == ownerIdentifier,
              current.fence == record.fence,
              current.expiresAt > clock.now()
        else {
            throw ComputerUseError.invalidSnapshot
        }
        return record
    }

    private func element(identifier: String, in captured: CapturedDesktopState) throws -> AccessibilityElementSnapshot {
        guard let element = captured.elements.first(where: { $0.identifier == identifier }) else {
            throw ComputerUseError.invalidArguments("The element identifier is not present in this snapshot.")
        }
        return element
    }

    private func pixelPoint(from object: [String: JSONValue]) throws -> PNGPixelPoint {
        var reader = ArgumentReader(object)
        let point = PNGPixelPoint(
            x: try reader.requiredNumber("x_px"),
            y: try reader.requiredNumber("y_px")
        )
        try reader.finish()
        return point
    }

    private func requiredReceipt(_ identifier: String) throws -> ActionReceipt {
        guard let receipt = try receipts.load(identifier: identifier) else {
            throw ComputerUseError.internalFailure("The durable action receipt is unavailable.")
        }
        return receipt
    }

    private func snapshotResult(_ envelope: SnapshotEnvelope, receiptIdentifier: String?) throws -> ToolExecutionResult {
        let captured = envelope.captured
        var header = "snapshot_id=\(envelope.snapshotIdentifier)\n"
        if let receiptIdentifier { header += "receipt_id=\(receiptIdentifier) action_state=applied\n" }
        header += "Coordinates use continuous PNG edge-space: width=\(captured.geometry.pngWidthPixels), height=\(captured.geometry.pngHeightPixels), origin=(0,0) top-left, x right, y down; require 0<=x<width and 0<=y<height.\n"
        let text = boundedUTF8(header + captured.accessibilityTree, maximumBytes: 16 * 1_024)
        return ToolExecutionResult(
            text: text,
            imagePNG: captured.screenshotPNG,
            protectedCarrier: try ProtectedComputerUseCarrier(snapshot: envelope)
        )
    }

    private func recoveredOutcomeResult(_ receipt: ActionReceipt) -> ToolExecutionResult {
        ToolExecutionResult(
            text: "Action receipt \(receipt.identifier) is already applied; it was not dispatched again. Call get_app_state."
        )
    }

    private func receiptErrorResult(_ receipt: ActionReceipt, message: String) -> ToolExecutionResult {
        ToolExecutionResult(text: "\(message) Receipt state: \(receipt.state.rawValue).", isError: true)
    }

    private func errorResult(_ error: ComputerUseError) -> ToolExecutionResult {
        ToolExecutionResult(text: "\(error.code): \(error.message)", isError: true)
    }
}

private func boundedUTF8(_ value: String, maximumBytes: Int) -> String {
    guard value.utf8.count > maximumBytes else { return value }
    var result = ""
    result.reserveCapacity(maximumBytes)
    var used = 0
    for character in value {
        let string = String(character)
        let count = string.utf8.count
        if used + count > maximumBytes { break }
        result.append(character)
        used += count
    }
    return result
}
