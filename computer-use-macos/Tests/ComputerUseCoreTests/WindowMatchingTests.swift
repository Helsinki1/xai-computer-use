import XCTest
@testable import ComputerUseCore

final class WindowMatchingTests: XCTestCase {
    private let captureFrame = GlobalScreenRect(x: 0, y: 30, width: 1792, height: 1090)

    func testNativeWindowNumberWinsDespiteGeometryDrift() {
        let candidates = [
            AccessibilityWindowCandidate(
                windowIdentifier: 105,
                frame: GlobalScreenRect(x: 0, y: 31, width: 1792, height: 1089),
                title: "Chrome"
            ),
            AccessibilityWindowCandidate(windowIdentifier: 106, frame: captureFrame, title: "Other")
        ]

        XCTAssertEqual(
            AccessibilityWindowMatcher.matchIndex(
                windowIdentifier: 105,
                frame: captureFrame,
                title: "Chrome",
                candidates: candidates
            ),
            0
        )
    }

    func testTolerantFrameMatchHandlesAccessibilityDecorationDifferences() {
        let candidates = [
            AccessibilityWindowCandidate(
                windowIdentifier: nil,
                frame: GlobalScreenRect(x: 0, y: 29, width: 1792, height: 1091),
                title: "Repository - Google Chrome - Profile"
            )
        ]

        XCTAssertEqual(
            AccessibilityWindowMatcher.matchIndex(
                windowIdentifier: 3565,
                frame: captureFrame,
                title: "Repository",
                candidates: candidates
            ),
            0
        )
    }

    func testNormalizedTitleDisambiguatesEqualFrames() {
        let candidates = [
            AccessibilityWindowCandidate(windowIdentifier: nil, frame: captureFrame, title: "One"),
            AccessibilityWindowCandidate(
                windowIdentifier: nil,
                frame: captureFrame,
                title: "Repository - Google Chrome - Profile"
            )
        ]

        XCTAssertEqual(
            AccessibilityWindowMatcher.matchIndex(
                windowIdentifier: 3565,
                frame: captureFrame,
                title: "repository",
                candidates: candidates
            ),
            1
        )
    }

    func testAmbiguousCandidatesStillFailClosed() {
        let candidates = [
            AccessibilityWindowCandidate(windowIdentifier: nil, frame: captureFrame, title: "Same"),
            AccessibilityWindowCandidate(windowIdentifier: nil, frame: captureFrame, title: "Same")
        ]

        XCTAssertNil(
            AccessibilityWindowMatcher.matchIndex(
                windowIdentifier: 3565,
                frame: captureFrame,
                title: "Same",
                candidates: candidates
            )
        )
    }
}
