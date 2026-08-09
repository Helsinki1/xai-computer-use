import AppKit
import ApplicationServices
import ComputerUseCore
import CoreGraphics
import CryptoKit
import Foundation
import ImageIO
@preconcurrency import ScreenCaptureKit
import UniformTypeIdentifiers

@MainActor
final class MacOSDesktopDriver: DesktopDriving {
    private static let maximumTreeNodes = 1_200
    private static let maximumTreeDepth = 64
    private static let maximumObservationBytes = 15_500
    private static let maximumAXStringBytes = 512
    private static let maximumAXActions = 16
    private static let maximumAXActionBytes = 128
    private static let accessibilityDeadlineNanoseconds: UInt64 = 150_000_000
    private static let windowRecoveryDelay: Duration = .milliseconds(700)
    private static let maximumPNGBytes = 900_000
    private static let maximumPNGDimension = 1_280
    private static let maximumPNGPixelCount = 1_638_400
    private static let screenCaptureDeadline: Duration = .seconds(5)

    private var elementHandles: [String: AXUIElement] = [:]
    private var capturedWindowHandle: AXUIElement?
    private var captureGeneration: UInt64 = 0

    func listApps() async throws -> [AppDescriptor] {
        let content = try await shareableContent()
        var windowIdentifiersByPID: [pid_t: [UInt32]] = [:]
        for window in content.windows where isCapturable(window) {
            guard let owner = window.owningApplication else { continue }
            windowIdentifiersByPID[owner.processID, default: []].append(window.windowID)
        }
        return NSWorkspace.shared.runningApplications
            .filter {
                $0.activationPolicy != .prohibited
                    && !$0.isTerminated
                    && !AppAccessPolicy.isBlocked(bundleIdentifier: $0.bundleIdentifier)
            }
            .map {
                AppDescriptor(
                    name: $0.localizedName ?? $0.bundleIdentifier ?? "Unknown",
                    bundleIdentifier: $0.bundleIdentifier,
                    processIdentifier: $0.processIdentifier,
                    isActive: $0.isActive,
                    windowIdentifiers: (windowIdentifiersByPID[$0.processIdentifier] ?? []).sorted()
                )
            }
            .sorted {
                if $0.isActive != $1.isActive { return $0.isActive }
                return $0.name.localizedCaseInsensitiveCompare($1.name) == .orderedAscending
            }
    }

    func capture(bundleIdentifier: String, windowIdentifier: UInt32?) async throws -> CapturedDesktopState {
        try AppAccessPolicy.requireAllowed(bundleIdentifier: bundleIdentifier)
        let generation = try beginCapture()
        let content = try await shareableContent()
        let candidates = NSWorkspace.shared.runningApplications.filter { app in
            guard app.activationPolicy != .prohibited, !app.isTerminated else { return false }
            return app.bundleIdentifier == bundleIdentifier
        }
        guard !candidates.isEmpty else {
            throw ComputerUseError.stateUnavailable("No running GUI application matches the requested bundle identifier.")
        }
        let selected: NSRunningApplication
        if let windowIdentifier {
            let ownerPID = content.windows.first(where: {
                $0.windowID == windowIdentifier && isCapturable($0)
            })?.owningApplication?.processID
            selected = candidates.first(where: { $0.processIdentifier == ownerPID })
                ?? candidates.first(where: \.isActive)
                ?? candidates[0]
        } else {
            selected = candidates.first(where: \.isActive) ?? candidates[0]
        }
        return try await capture(
            application: selected,
            requestedWindowIdentifier: windowIdentifier,
            content: content,
            generation: generation
        )
    }

    func capture(processIdentifier: Int32, windowIdentifier: UInt32?) async throws -> CapturedDesktopState {
        let generation = try beginCapture()
        guard let app = NSRunningApplication(processIdentifier: processIdentifier), !app.isTerminated else {
            throw ComputerUseError.stateUnavailable("The snapshot application is no longer running.")
        }
        try AppAccessPolicy.requireAllowed(bundleIdentifier: app.bundleIdentifier)
        let content = try await shareableContent()
        return try await capture(
            application: app,
            requestedWindowIdentifier: windowIdentifier,
            content: content,
            generation: generation
        )
    }

    func click(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        point: GlobalScreenPoint,
        button: MouseButton,
        count: Int
    ) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        try Task.checkCancellation()
        guard let source = CGEventSource(stateID: .combinedSessionState) else {
            throw ComputerUseError.stateUnavailable("Could not create a targeted mouse event source.")
        }
        let cgPoint = CGPoint(x: point.x, y: point.y)
        let properties = mouseProperties(button)
        for clickIndex in 1...count {
            let moved = try mouseEvent(
                type: .mouseMoved,
                source: source,
                point: cgPoint,
                button: properties.button,
                clickState: clickIndex
            )
            let down = try mouseEvent(
                type: properties.down,
                source: source,
                point: cgPoint,
                button: properties.button,
                clickState: clickIndex
            )
            let up = try mouseEvent(
                type: properties.up,
                source: source,
                point: cgPoint,
                button: properties.button,
                clickState: clickIndex
            )
            moved.postToPid(app.processIdentifier)
            down.postToPid(app.processIdentifier)
            up.postToPid(app.processIdentifier)
        }
    }

    func performAccessibilityAction(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        driverToken: String,
        action: String
    ) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        let element = try validatedElement(
            driverToken: driverToken,
            app: app,
            expectedGeometry: expectedGeometry
        )
        let actions = copyActionNames(element)
        guard let exact = actions.first(where: { $0.caseInsensitiveCompare(action) == .orderedSame }) else {
            throw ComputerUseError.invalidArguments("The accessibility action is no longer available.")
        }
        try Task.checkCancellation()
        let result = AXUIElementPerformAction(element, exact as CFString)
        guard result == .success else {
            throw ComputerUseError.stateUnavailable("The target application rejected the accessibility action.")
        }
    }

    func scroll(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        point: GlobalScreenPoint,
        deltaX: Double,
        deltaY: Double
    ) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        guard deltaX.isFinite, deltaY.isFinite,
              let event = CGEvent(
                scrollWheelEvent2Source: nil,
                units: .line,
                wheelCount: 2,
                wheel1: clampedInt32(deltaY),
                wheel2: clampedInt32(deltaX),
                wheel3: 0
              )
        else {
            throw ComputerUseError.invalidArguments("Invalid scroll delta.")
        }
        try Task.checkCancellation()
        event.location = CGPoint(x: point.x, y: point.y)
        event.postToPid(app.processIdentifier)
    }

    func drag(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        from: GlobalScreenPoint,
        to: GlobalScreenPoint
    ) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        guard let source = CGEventSource(stateID: .combinedSessionState) else {
            throw ComputerUseError.stateUnavailable("Could not create a targeted drag event source.")
        }
        let start = CGPoint(x: from.x, y: from.y)
        let end = CGPoint(x: to.x, y: to.y)
        let moved = try mouseEvent(type: .mouseMoved, source: source, point: start, button: .left, clickState: 1)
        let down = try mouseEvent(type: .leftMouseDown, source: source, point: start, button: .left, clickState: 1)
        let up = try mouseEvent(type: .leftMouseUp, source: source, point: start, button: .left, clickState: 1)
        try Task.checkCancellation()
        moved.postToPid(app.processIdentifier)
        down.postToPid(app.processIdentifier)
        var buttonIsDown = true
        defer {
            if buttonIsDown { up.postToPid(app.processIdentifier) }
        }
        for step in 1...24 {
            let progress = Double(step) / 24
            let point = CGPoint(
                x: start.x + (end.x - start.x) * progress,
                y: start.y + (end.y - start.y) * progress
            )
            let dragged = try mouseEvent(
                type: .leftMouseDragged,
                source: source,
                point: point,
                button: .left,
                clickState: 1
            )
            up.location = point
            dragged.postToPid(app.processIdentifier)
            try await Task.sleep(for: .milliseconds(5))
        }
        up.location = end
        up.postToPid(app.processIdentifier)
        buttonIsDown = false
    }

    func typeText(app: AppTarget, expectedGeometry: WindowGeometry, text: String) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        _ = try editableFocusedElement(app: app, expectedGeometry: expectedGeometry)
        for chunk in unicodeChunks(text, maximumUTF16Units: 64) {
            guard let down = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: true),
                  let up = CGEvent(keyboardEventSource: nil, virtualKey: 0, keyDown: false)
            else {
                throw ComputerUseError.stateUnavailable("Could not create a targeted keyboard event.")
            }
            var mutable = chunk
            mutable.withUnsafeMutableBufferPointer { buffer in
                guard let base = buffer.baseAddress else { return }
                down.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: base)
                up.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: base)
            }
            try Task.checkCancellation()
            down.postToPid(app.processIdentifier)
            up.postToPid(app.processIdentifier)
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    func pressKey(app: AppTarget, expectedGeometry: WindowGeometry, specification: String) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        try requireCapturedWindowIsFocused(app: app, expectedGeometry: expectedGeometry)
        let parsed = try KeySpecification.parse(specification)
        var activeFlags: CGEventFlags = []
        var modifierDownEvents: [CGEvent] = []
        for modifier in parsed.modifiers {
            activeFlags.insert(modifier.flag)
            guard let event = CGEvent(keyboardEventSource: nil, virtualKey: modifier.keyCode, keyDown: true) else {
                throw ComputerUseError.stateUnavailable("Could not create a modifier event.")
            }
            event.flags = activeFlags
            modifierDownEvents.append(event)
        }

        guard let down = CGEvent(keyboardEventSource: nil, virtualKey: parsed.keyCode, keyDown: true),
              let up = CGEvent(keyboardEventSource: nil, virtualKey: parsed.keyCode, keyDown: false)
        else {
            throw ComputerUseError.stateUnavailable("Could not create the key event.")
        }
        down.flags = activeFlags
        up.flags = activeFlags

        var modifierUpEvents: [CGEvent] = []
        var releaseFlags = activeFlags
        for modifier in parsed.modifiers.reversed() {
            guard let event = CGEvent(keyboardEventSource: nil, virtualKey: modifier.keyCode, keyDown: false) else {
                throw ComputerUseError.stateUnavailable("Could not create a modifier release event.")
            }
            event.flags = releaseFlags
            modifierUpEvents.append(event)
            releaseFlags.remove(modifier.flag)
        }
        try Task.checkCancellation()
        modifierDownEvents.forEach { $0.postToPid(app.processIdentifier) }
        down.postToPid(app.processIdentifier)
        up.postToPid(app.processIdentifier)
        modifierUpEvents.forEach { $0.postToPid(app.processIdentifier) }
    }

    func setValue(
        app: AppTarget,
        expectedGeometry: WindowGeometry,
        driverToken: String,
        value: String
    ) async throws {
        try await prepare(app)
        try await revalidate(app: app, expectedGeometry: expectedGeometry)
        let element = try validatedElement(
            driverToken: driverToken,
            app: app,
            expectedGeometry: expectedGeometry
        )
        var settable = DarwinBoolean(false)
        guard AXUIElementIsAttributeSettable(element, kAXValueAttribute as CFString, &settable) == .success,
              settable.boolValue
        else {
            throw ComputerUseError.invalidArguments("The accessibility value is no longer settable.")
        }
        try Task.checkCancellation()
        let result = AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, value as CFString)
        guard result == .success else {
            throw ComputerUseError.stateUnavailable("The target application rejected the accessibility value.")
        }
    }

    private func capture(
        application: NSRunningApplication,
        requestedWindowIdentifier: UInt32?,
        content: SCShareableContent,
        generation: UInt64,
        allowRecovery: Bool = true
    ) async throws -> CapturedDesktopState {
        guard AXIsProcessTrusted() else {
            throw ComputerUseError.permissionDenied("Accessibility permission is required for Grok Computer Use.app.")
        }
        guard CGPreflightScreenCaptureAccess() else {
            throw ComputerUseError.permissionDenied("Screen Recording permission is required for Grok Computer Use.app.")
        }

        let pid = application.processIdentifier
        let appElement = AXUIElementCreateApplication(pid)
        AXUIElementSetMessagingTimeout(appElement, 0.15)
        enableBestEffortAccessibilityModes(appElement)
        let accessibilityWindows = copyElements(appElement, attribute: kAXWindowsAttribute)
        let focusedWindow = copyElement(appElement, attribute: kAXFocusedWindowAttribute)
        let selectionHint = focusedWindow ?? accessibilityWindows.first
        let focusedTitle = selectionHint.flatMap { copyString($0, attribute: kAXTitleAttribute) }
        let focusedFrame = selectionHint.flatMap(copyFrame)

        let candidates = content.windows.filter { window in
            window.owningApplication?.processID == pid && isCapturable(window)
        }
        let window: SCWindow?
        if let requestedWindowIdentifier {
            window = candidates.first(where: { $0.windowID == requestedWindowIdentifier })
                ?? chooseWindow(candidates, title: focusedTitle, frame: focusedFrame)
        } else {
            window = chooseWindow(candidates, title: focusedTitle, frame: focusedFrame)
        }
        guard let window else {
            if allowRecovery {
                try await recoverVisibleWindow(application: application, appElement: appElement)
                return try await capture(
                    application: application,
                    requestedWindowIdentifier: requestedWindowIdentifier,
                    content: try await shareableContent(),
                    generation: generation,
                    allowRecovery: false
                )
            }
            throw ComputerUseError.stateUnavailable("The target application has no capturable on-screen window.")
        }
        let bounds = window.frame
        guard bounds.origin.x.isFinite,
              bounds.origin.y.isFinite,
              bounds.width.isFinite,
              bounds.height.isFinite,
              bounds.width > 0,
              bounds.height > 0
        else {
            throw ComputerUseError.stateUnavailable("ScreenCaptureKit returned invalid window geometry.")
        }
        guard let accessibilityWindow = matchingAccessibilityWindow(
            matching: window,
            candidates: accessibilityWindows
        ) else {
            if allowRecovery {
                try await recoverVisibleWindow(application: application, appElement: appElement)
                return try await capture(
                    application: application,
                    requestedWindowIdentifier: requestedWindowIdentifier,
                    content: try await shareableContent(),
                    generation: generation,
                    allowRecovery: false
                )
            }
            throw ComputerUseError.stateUnavailable(
                "Could not pair ScreenCaptureKit window \(window.windowID) with an accessibility window for pid \(pid) (AX windows: \(accessibilityWindows.count))."
            )
        }

        let image = try await capture(window: window)
        try await requireCurrentWindow(
            processIdentifier: pid,
            bundleIdentifier: application.bundleIdentifier,
            windowIdentifier: window.windowID,
            expectedBounds: bounds
        )
        let bounded = try boundedCanonicalPNG(image)
        let accessibility = renderAccessibility(accessibilityWindow)
        guard generation == captureGeneration else {
            throw ComputerUseError.invalidSnapshot
        }
        capturedWindowHandle = accessibilityWindow
        elementHandles = accessibility.handles
        guard let bundleIdentifier = application.bundleIdentifier,
              (3...255).contains(bundleIdentifier.utf8.count)
        else {
            throw ComputerUseError.stateUnavailable("The target application has no valid bundle identifier.")
        }
        let target = AppTarget(
            name: application.localizedName ?? application.bundleIdentifier ?? "Unknown",
            bundleIdentifier: bundleIdentifier,
            processIdentifier: pid
        )
        return CapturedDesktopState(
            app: target,
            windowTitle: window.title ?? copyString(accessibilityWindow, attribute: kAXTitleAttribute),
            geometry: WindowGeometry(
                windowIdentifier: window.windowID,
                globalBoundsPoints: GlobalScreenRect(
                    x: bounds.origin.x,
                    y: bounds.origin.y,
                    width: bounds.width,
                    height: bounds.height
                ),
                pngWidthPixels: bounded.width,
                pngHeightPixels: bounded.height
            ),
            screenshotPNG: bounded.data,
            screenshotSHA256: SHA256.hash(data: bounded.data).map { String(format: "%02x", $0) }.joined(),
            accessibilityTree: accessibility.tree,
            elements: accessibility.elements
        )
    }

    private func capture(window: SCWindow) async throws -> CGImage {
        let filter = SCContentFilter(desktopIndependentWindow: window)
        let configuration = SCStreamConfiguration()
        let authoritativeScale = CGFloat(filter.pointPixelScale)
        guard authoritativeScale.isFinite, authoritativeScale > 0,
              filter.contentRect.width > 0, filter.contentRect.height > 0
        else {
            throw ComputerUseError.stateUnavailable("ScreenCaptureKit did not provide a valid capture scale and content rectangle.")
        }
        configuration.width = max(1, Int(ceil(filter.contentRect.width * authoritativeScale)))
        configuration.height = max(1, Int(ceil(filter.contentRect.height * authoritativeScale)))
        configuration.scalesToFit = false
        configuration.ignoreShadowsSingleWindow = true
        configuration.showsCursor = false
        let captured: UncheckedScreenCaptureValue<CGImage> = try await withScreenCaptureDeadline {
            UncheckedScreenCaptureValue(value:
                try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
            )
        }
        return captured.value
    }

    private func boundedCanonicalPNG(_ source: CGImage) throws -> (data: Data, width: Int, height: Int) {
        guard source.width > 0, source.height > 0 else {
            throw ComputerUseError.stateUnavailable("ScreenCaptureKit returned an empty image.")
        }
        let dimensionScale = min(
            1,
            Double(Self.maximumPNGDimension) / Double(source.width),
            Double(Self.maximumPNGDimension) / Double(source.height)
        )
        let pixelScale = min(
            1,
            sqrt(Double(Self.maximumPNGPixelCount) / Double(source.width * source.height))
        )
        var scale = min(dimensionScale, pixelScale)
        var previousSize: (Int, Int)?

        while true {
            let width = max(1, Int(floor(Double(source.width) * scale)))
            let height = max(1, Int(floor(Double(source.height) * scale)))
            if previousSize?.0 == width, previousSize?.1 == height {
                scale *= 0.8
                continue
            }
            previousSize = (width, height)

            let canonical = try canonicalSRGBA8Image(source, width: width, height: height)
            let png = try encodePNG(canonical)
            if png.count <= Self.maximumPNGBytes,
               width <= Self.maximumPNGDimension,
               height <= Self.maximumPNGDimension,
               width * height <= Self.maximumPNGPixelCount
            {
                return (png, width, height)
            }
            guard width > 1 || height > 1 else {
                throw ComputerUseError.stateUnavailable("The screenshot could not be represented within the bounded PNG contract.")
            }
            scale *= 0.8
        }
    }

    private func canonicalSRGBA8Image(_ source: CGImage, width: Int, height: Int) throws -> CGImage {
        guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB),
              let context = CGContext(
                data: nil,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: width * 4,
                space: colorSpace,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
              )
        else {
            throw ComputerUseError.stateUnavailable("Could not allocate the canonical screenshot buffer.")
        }
        context.interpolationQuality = .high
        context.setBlendMode(.copy)
        context.draw(source, in: CGRect(x: 0, y: 0, width: width, height: height))
        guard let image = context.makeImage() else {
            throw ComputerUseError.stateUnavailable("Could not create the canonical screenshot image.")
        }
        return image
    }

    private func encodePNG(_ image: CGImage) throws -> Data {
        let data = NSMutableData()
        guard let destination = CGImageDestinationCreateWithData(
            data,
            UTType.png.identifier as CFString,
            1,
            nil
        ) else {
            throw ComputerUseError.stateUnavailable("Could not create a PNG encoder.")
        }
        CGImageDestinationAddImage(destination, image, nil)
        guard CGImageDestinationFinalize(destination) else {
            throw ComputerUseError.stateUnavailable("Could not finalize the PNG screenshot.")
        }
        return data as Data
    }

    private func shareableContent() async throws -> SCShareableContent {
        guard CGPreflightScreenCaptureAccess() else {
            throw ComputerUseError.permissionDenied("Screen Recording permission is required for Grok Computer Use.app.")
        }
        let content: UncheckedScreenCaptureValue<SCShareableContent> = try await withScreenCaptureDeadline {
            UncheckedScreenCaptureValue(value:
                try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)
            )
        }
        return content.value
    }

    private func withScreenCaptureDeadline<Value: Sendable>(
        _ operation: @escaping @MainActor () async throws -> Value
    ) async throws -> Value {
        let race = AsyncDeadlineRace<Value>()
        let deadline = Self.screenCaptureDeadline
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                race.install(continuation)
                let operationTask = Task { @MainActor in
                    do {
                        race.resolve(.success(try await operation()))
                    } catch {
                        race.resolve(.failure(error))
                    }
                }
                let timerTask = Task { @MainActor in
                    do {
                        try await Task.sleep(for: deadline)
                        race.resolve(.failure(ComputerUseError.stateUnavailable(
                            "ScreenCaptureKit exceeded its five-second deadline."
                        )))
                    } catch {
                        // The timer task is best-effort; another result may already have won.
                    }
                }
                race.installTasks(operationTask, timerTask)
            }
        } onCancel: {
            Task { @MainActor in
                race.resolve(.failure(CancellationError()))
            }
        }
    }

    private func isCapturable(_ window: SCWindow) -> Bool {
        window.isOnScreen && window.frame.width > 1 && window.frame.height > 1
    }

    private func matchingAccessibilityWindow(
        matching window: SCWindow,
        candidates: [AXUIElement]
    ) -> AXUIElement? {
        let descriptions = candidates.map {
            AccessibilityWindowCandidate(
                windowIdentifier: copyUInt32($0, attribute: "AXWindowNumber"),
                frame: copyFrame($0).map(globalScreenRect),
                title: copyString($0, attribute: kAXTitleAttribute)
            )
        }
        guard let index = AccessibilityWindowMatcher.matchIndex(
            windowIdentifier: window.windowID,
            frame: globalScreenRect(window.frame),
            title: window.title,
            candidates: descriptions
        ) else { return nil }
        return candidates[index]
    }

    private func recoverVisibleWindow(
        application: NSRunningApplication,
        appElement: AXUIElement
    ) async throws {
        _ = application.unhide()
        _ = application.activate(options: [.activateAllWindows])

        let candidate = copyElement(appElement, attribute: kAXFocusedWindowAttribute)
            ?? copyElements(appElement, attribute: kAXWindowsAttribute).first
        if let candidate {
            if copyBool(candidate, attribute: kAXMinimizedAttribute) == true {
                _ = AXUIElementSetAttributeValue(
                    candidate,
                    kAXMinimizedAttribute as CFString,
                    kCFBooleanFalse
                )
            }
            _ = AXUIElementPerformAction(candidate, kAXRaiseAction as CFString)
            _ = AXUIElementSetAttributeValue(
                candidate,
                kAXMainAttribute as CFString,
                kCFBooleanTrue
            )
            _ = AXUIElementSetAttributeValue(
                candidate,
                kAXFocusedAttribute as CFString,
                kCFBooleanTrue
            )
        }

        try await Task.sleep(for: Self.windowRecoveryDelay)
    }

    private func revalidate(app: AppTarget, expectedGeometry: WindowGeometry) async throws {
        let bounds = expectedGeometry.globalBoundsPoints
        try await requireCurrentWindow(
            processIdentifier: app.processIdentifier,
            bundleIdentifier: app.bundleIdentifier,
            windowIdentifier: expectedGeometry.windowIdentifier,
            expectedBounds: CGRect(x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height)
        )
    }

    private func requireCurrentWindow(
        processIdentifier: pid_t,
        bundleIdentifier: String?,
        windowIdentifier: UInt32,
        expectedBounds: CGRect
    ) async throws {
        let content = try await shareableContent()
        guard let window = content.windows.first(where: { $0.windowID == windowIdentifier }),
              isCapturable(window),
              let owner = window.owningApplication,
              owner.processID == processIdentifier,
              bundleIdentifier == nil || owner.bundleIdentifier == bundleIdentifier,
              framesAreEqual(window.frame, expectedBounds)
        else {
            throw ComputerUseError.invalidSnapshot
        }
    }

    private func framesAreEqual(_ left: CGRect, _ right: CGRect) -> Bool {
        AccessibilityWindowMatcher.framesMatch(globalScreenRect(left), globalScreenRect(right))
    }

    private func globalScreenRect(_ rect: CGRect) -> GlobalScreenRect {
        GlobalScreenRect(
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.width,
            height: rect.height
        )
    }

    private func enableBestEffortAccessibilityModes(_ appElement: AXUIElement) {
        _ = AXUIElementSetAttributeValue(
            appElement,
            "AXManualAccessibility" as CFString,
            kCFBooleanTrue
        )
        _ = AXUIElementSetAttributeValue(
            appElement,
            "AXEnhancedUserInterface" as CFString,
            kCFBooleanTrue
        )
    }

    private func chooseWindow(_ windows: [SCWindow], title: String?, frame: CGRect?) -> SCWindow? {
        windows.min { left, right in
            let leftScore = windowScore(left, title: title, frame: frame)
            let rightScore = windowScore(right, title: title, frame: frame)
            return leftScore == rightScore ? left.windowID < right.windowID : leftScore < rightScore
        }
    }

    private func windowScore(_ window: SCWindow, title: String?, frame: CGRect?) -> Double {
        var score = 0.0
        if let title, !title.isEmpty, window.title != title { score += 1_000_000 }
        if let frame {
            score += abs(window.frame.minX - frame.minX)
                + abs(window.frame.minY - frame.minY)
                + abs(window.frame.width - frame.width)
                + abs(window.frame.height - frame.height)
        } else {
            score -= window.frame.width * window.frame.height / 1_000
        }
        return score
    }

    private func renderAccessibility(
        _ root: AXUIElement
    ) -> (tree: String, elements: [AccessibilityElementSnapshot], handles: [String: AXUIElement]) {
        var handles: [String: AXUIElement] = [:]
        var elements: [AccessibilityElementSnapshot] = []
        var lines: [String] = []
        var observationBytes = 0
        var exhausted = false
        let deadline = DispatchTime.now().uptimeNanoseconds + Self.accessibilityDeadlineNanoseconds

        func visit(_ element: AXUIElement, depth: Int) {
            guard !exhausted,
                  elements.count < Self.maximumTreeNodes,
                  depth <= Self.maximumTreeDepth,
                  DispatchTime.now().uptimeNanoseconds < deadline
            else {
                exhausted = true
                return
            }
            let identifier = "e\(elements.count + 1)"
            let token = UUID().uuidString.lowercased()
            let role = boundedUTF8(copyString(element, attribute: kAXRoleAttribute) ?? "AXUnknown", maximumBytes: 128)
            let subrole = copyString(element, attribute: kAXSubroleAttribute)
            let secure = subrole == kAXSecureTextFieldSubrole as String
            let rawLabel = copyString(element, attribute: kAXTitleAttribute)
                ?? copyString(element, attribute: kAXDescriptionAttribute)
            let label = rawLabel.map { boundedUTF8($0, maximumBytes: Self.maximumAXStringBytes) }
            let value = secure
                ? "<redacted secure value>"
                : copyDisplayValue(element).map { boundedUTF8($0, maximumBytes: Self.maximumAXStringBytes) }
            let frame = copyFrame(element).map {
                GlobalScreenRect(x: $0.minX, y: $0.minY, width: $0.width, height: $0.height)
            }
            let actions = copyActionNames(element)
                .prefix(Self.maximumAXActions)
                .map { boundedUTF8($0, maximumBytes: Self.maximumAXActionBytes) }
            var settable = DarwinBoolean(false)
            let isSettable = AXUIElementIsAttributeSettable(element, kAXValueAttribute as CFString, &settable) == .success
                && settable.boolValue
                && !secure

            var fields = ["[\(identifier)]", role]
            if let label, !label.isEmpty { fields.append("label=\"\(treeText(label))\"") }
            if let value, !value.isEmpty { fields.append("value=\"\(treeText(value))\"") }
            if !actions.isEmpty { fields.append("actions=\(actions.joined(separator: ","))") }
            if isSettable { fields.append("settable=true") }
            let line = String(repeating: "  ", count: depth) + fields.joined(separator: " ")
            let lineBytes = line.utf8.count + 1
            guard observationBytes + lineBytes <= Self.maximumObservationBytes else {
                exhausted = true
                return
            }
            observationBytes += lineBytes
            lines.append(line)
            handles[token] = element
            elements.append(AccessibilityElementSnapshot(
                identifier: identifier,
                role: role,
                label: label,
                value: value,
                frame: frame,
                actions: actions,
                isValueSettable: isSettable,
                driverToken: token
            ))

            for child in copyElements(element, attribute: kAXChildrenAttribute) {
                visit(child, depth: depth + 1)
                if elements.count >= Self.maximumTreeNodes { break }
            }
        }

        visit(root, depth: 0)
        if exhausted {
            let marker = "… accessibility observation truncated by a deterministic safety bound"
            if observationBytes + marker.utf8.count + 1 <= Self.maximumObservationBytes {
                lines.append(marker)
            }
        }
        return (lines.joined(separator: "\n"), elements, handles)
    }

    private func beginCapture() throws -> UInt64 {
        guard captureGeneration < UInt64.max else {
            throw ComputerUseError.internalFailure("The capture generation space is exhausted.")
        }
        captureGeneration += 1
        return captureGeneration
    }

    private func prepare(_ app: AppTarget) async throws {
        guard let running = NSRunningApplication(processIdentifier: app.processIdentifier),
              !running.isTerminated,
              app.bundleIdentifier == nil || running.bundleIdentifier == app.bundleIdentifier
        else {
            throw ComputerUseError.stateUnavailable("The snapshot application is no longer running.")
        }
        _ = running.activate(options: [.activateAllWindows])
        try await Task.sleep(for: .milliseconds(80))
    }

    private func validatedElement(
        driverToken: String,
        app: AppTarget,
        expectedGeometry: WindowGeometry
    ) throws -> AXUIElement {
        let capturedWindow = try requireCapturedAccessibilityWindow(
            app: app,
            expectedGeometry: expectedGeometry
        )
        guard let element = elementHandles[driverToken],
              processIdentifier(of: element) == app.processIdentifier,
              CFEqual(element, capturedWindow)
                || copyElement(element, attribute: kAXWindowAttribute).map({ CFEqual($0, capturedWindow) }) == true
        else {
            throw ComputerUseError.invalidSnapshot
        }
        return element
    }

    private func editableFocusedElement(
        app: AppTarget,
        expectedGeometry: WindowGeometry
    ) throws -> AXUIElement {
        let capturedWindow = try requireCapturedAccessibilityWindow(
            app: app,
            expectedGeometry: expectedGeometry
        )
        let appElement = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(appElement, 0.15)
        guard let focused = copyElement(appElement, attribute: kAXFocusedUIElementAttribute),
              processIdentifier(of: focused) == app.processIdentifier,
              let focusedWindow = copyElement(focused, attribute: kAXWindowAttribute),
              CFEqual(focusedWindow, capturedWindow),
              copyBool(focused, attribute: kAXEnabledAttribute) != false
        else {
            throw ComputerUseError.invalidSnapshot
        }
        var settable = DarwinBoolean(false)
        guard AXUIElementIsAttributeSettable(focused, kAXValueAttribute as CFString, &settable) == .success,
              settable.boolValue
        else {
            throw ComputerUseError.invalidArguments("The captured window does not have an editable focused element.")
        }
        return focused
    }

    private func requireCapturedWindowIsFocused(
        app: AppTarget,
        expectedGeometry: WindowGeometry
    ) throws {
        let capturedWindow = try requireCapturedAccessibilityWindow(
            app: app,
            expectedGeometry: expectedGeometry
        )
        let appElement = AXUIElementCreateApplication(app.processIdentifier)
        AXUIElementSetMessagingTimeout(appElement, 0.15)
        guard let focusedWindow = copyElement(appElement, attribute: kAXFocusedWindowAttribute),
              CFEqual(focusedWindow, capturedWindow)
        else {
            throw ComputerUseError.invalidSnapshot
        }
    }

    private func requireCapturedAccessibilityWindow(
        app: AppTarget,
        expectedGeometry: WindowGeometry
    ) throws -> AXUIElement {
        let bounds = expectedGeometry.globalBoundsPoints
        let expectedBounds = CGRect(x: bounds.x, y: bounds.y, width: bounds.width, height: bounds.height)
        guard let capturedWindow = capturedWindowHandle,
              processIdentifier(of: capturedWindow) == app.processIdentifier,
              copyFrame(capturedWindow).map({ framesAreEqual($0, expectedBounds) }) == true,
              copyUInt32(capturedWindow, attribute: "AXWindowNumber").map({
                  $0 == expectedGeometry.windowIdentifier
              }) != false
        else {
            throw ComputerUseError.invalidSnapshot
        }
        return capturedWindow
    }

    private func mouseProperties(_ button: MouseButton) -> (button: CGMouseButton, down: CGEventType, up: CGEventType) {
        switch button {
        case .left: (.left, .leftMouseDown, .leftMouseUp)
        case .right: (.right, .rightMouseDown, .rightMouseUp)
        case .middle: (.center, .otherMouseDown, .otherMouseUp)
        }
    }

    private func mouseEvent(
        type: CGEventType,
        source: CGEventSource,
        point: CGPoint,
        button: CGMouseButton,
        clickState: Int
    ) throws -> CGEvent {
        guard let event = CGEvent(mouseEventSource: source, mouseType: type, mouseCursorPosition: point, mouseButton: button) else {
            throw ComputerUseError.stateUnavailable("Could not create a targeted mouse event.")
        }
        event.setIntegerValueField(.mouseEventClickState, value: Int64(clickState))
        return event
    }

    private func clampedInt32(_ value: Double) -> Int32 {
        Int32(min(Double(Int32.max), max(Double(Int32.min), value.rounded(.toNearestOrAwayFromZero))))
    }

    private func unicodeChunks(_ text: String, maximumUTF16Units: Int) -> [[UniChar]] {
        var result: [[UniChar]] = []
        var current: [UniChar] = []
        for character in text {
            let units = Array(String(character).utf16)
            if !current.isEmpty, current.count + units.count > maximumUTF16Units {
                result.append(current)
                current.removeAll(keepingCapacity: true)
            }
            current.append(contentsOf: units)
        }
        if !current.isEmpty { result.append(current) }
        return result
    }
}

@MainActor
private final class AsyncDeadlineRace<Value: Sendable> {
    private var continuation: CheckedContinuation<Value, Error>?
    private var result: Result<Value, Error>?
    private var tasks: [Task<Void, Never>] = []

    func install(_ continuation: CheckedContinuation<Value, Error>) {
        if let result {
            continuation.resume(with: result)
        } else {
            self.continuation = continuation
        }
    }

    func installTasks(_ tasks: Task<Void, Never>...) {
        if result == nil {
            self.tasks = tasks
        } else {
            tasks.forEach { $0.cancel() }
        }
    }

    func resolve(_ result: Result<Value, Error>) {
        guard self.result == nil else {
            return
        }
        self.result = result
        let continuation = continuation
        self.continuation = nil
        let tasks = tasks
        self.tasks.removeAll()
        continuation?.resume(with: result)
        tasks.forEach { $0.cancel() }
    }
}

// ScreenCaptureKit types are used exclusively on the main actor by this driver,
// but the SDK does not yet declare them Sendable.
private struct UncheckedScreenCaptureValue<Value>: @unchecked Sendable {
    let value: Value
}

private func copyElement(_ element: AXUIElement, attribute: String) -> AXUIElement? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
          let value,
          CFGetTypeID(value) == AXUIElementGetTypeID()
    else { return nil }
    return (value as! AXUIElement)
}

private func copyElements(_ element: AXUIElement, attribute: String) -> [AXUIElement] {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
          let result = value as? [AXUIElement]
    else { return [] }
    return result
}

private func copyString(_ element: AXUIElement, attribute: String) -> String? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success else { return nil }
    return value as? String
}

private func copyUInt32(_ element: AXUIElement, attribute: String) -> UInt32? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
          let number = value as? NSNumber
    else {
        return nil
    }
    return UInt32(exactly: number.uint64Value)
}

private func copyBool(_ element: AXUIElement, attribute: String) -> Bool? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, attribute as CFString, &value) == .success,
          let number = value as? NSNumber
    else {
        return nil
    }
    return number.boolValue
}

private func processIdentifier(of element: AXUIElement) -> pid_t? {
    var processIdentifier = pid_t(0)
    guard AXUIElementGetPid(element, &processIdentifier) == .success,
          processIdentifier > 0
    else {
        return nil
    }
    return processIdentifier
}

private func copyDisplayValue(_ element: AXUIElement) -> String? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXValueAttribute as CFString, &value) == .success,
          let value
    else { return nil }
    if let string = value as? String { return string }
    if let number = value as? NSNumber { return number.stringValue }
    return nil
}

private func copyFrame(_ element: AXUIElement) -> CGRect? {
    var positionValue: CFTypeRef?
    var sizeValue: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue) == .success,
          AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue) == .success,
          let positionValue,
          let sizeValue,
          CFGetTypeID(positionValue) == AXValueGetTypeID(),
          CFGetTypeID(sizeValue) == AXValueGetTypeID()
    else { return nil }
    let positionAX = positionValue as! AXValue
    let sizeAX = sizeValue as! AXValue
    var position = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(positionAX, .cgPoint, &position),
          AXValueGetValue(sizeAX, .cgSize, &size),
          position.x.isFinite,
          position.y.isFinite,
          size.width.isFinite,
          size.height.isFinite,
          size.width >= 0,
          size.height >= 0
    else { return nil }
    return CGRect(origin: position, size: size)
}

private func copyActionNames(_ element: AXUIElement) -> [String] {
    var value: CFArray?
    guard AXUIElementCopyActionNames(element, &value) == .success, let value else { return [] }
    return value as? [String] ?? []
}

private func treeText(_ value: String) -> String {
    boundedUTF8(
        value.replacingOccurrences(of: "\n", with: " ").replacingOccurrences(of: "\"", with: "\\\""),
        maximumBytes: 512
    )
}

private func boundedUTF8(_ value: String, maximumBytes: Int) -> String {
    guard value.utf8.count > maximumBytes else { return value }
    var result = ""
    var used = 0
    for character in value {
        let text = String(character)
        let count = text.utf8.count
        if used + count > maximumBytes { break }
        result.append(character)
        used += count
    }
    return result
}

private struct KeyModifier {
    let keyCode: CGKeyCode
    let flag: CGEventFlags
}

private struct KeySpecification {
    let modifiers: [KeyModifier]
    let keyCode: CGKeyCode

    static func parse(_ value: String) throws -> KeySpecification {
        let components = value.split(separator: "+", omittingEmptySubsequences: false).map {
            $0.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        }
        guard let rawKeyName = components.last, !rawKeyName.isEmpty else {
            throw ComputerUseError.invalidArguments("Invalid key specification.")
        }
        let keyName = try KeyboardKeyName.canonicalize(rawKeyName)
        var modifiers: [KeyModifier] = []
        var seen = Set<String>()
        for name in components.dropLast() {
            let canonical: String
            let modifier: KeyModifier
            switch name {
            case "cmd", "command", "meta", "super":
                canonical = "command"; modifier = KeyModifier(keyCode: 55, flag: .maskCommand)
            case "ctrl", "control":
                canonical = "control"; modifier = KeyModifier(keyCode: 59, flag: .maskControl)
            case "alt", "option":
                canonical = "option"; modifier = KeyModifier(keyCode: 58, flag: .maskAlternate)
            case "shift":
                canonical = "shift"; modifier = KeyModifier(keyCode: 56, flag: .maskShift)
            case "fn":
                canonical = "fn"; modifier = KeyModifier(keyCode: 63, flag: .maskSecondaryFn)
            default:
                throw ComputerUseError.invalidArguments("Unsupported key modifier.")
            }
            if seen.insert(canonical).inserted { modifiers.append(modifier) }
        }
        guard let keyCode = keyCodes[keyName] else {
            throw ComputerUseError.invalidArguments("Unsupported key name.")
        }
        return KeySpecification(modifiers: modifiers, keyCode: keyCode)
    }

    private static let keyCodes: [String: CGKeyCode] = [
        "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
        "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15,
        "y": 16, "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22,
        "5": 23, "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29,
        "]": 30, "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "return": 36,
        "enter": 36, "l": 37, "j": 38, "'": 39, "k": 40, ";": 41, "\\": 42,
        ",": 43, "/": 44, "n": 45, "m": 46, ".": 47, "tab": 48, "space": 49,
        "backspace": 51, "delete": 51, "escape": 53, "esc": 53, "left": 123,
        "right": 124, "down": 125, "up": 126, "home": 115, "end": 119,
        "pageup": 116, "pagedown": 121, "forwarddelete": 117,
    ]
}
