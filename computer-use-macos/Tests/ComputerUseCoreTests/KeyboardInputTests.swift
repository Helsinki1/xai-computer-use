import XCTest
@testable import ComputerUseCore

final class KeyboardInputTests: XCTestCase {
    func testArrowAliasesCanonicalize() throws {
        for alias in ["right", "RightArrow", "ArrowRight", "right_arrow", "right arrow"] {
            XCTAssertEqual(try KeyboardKeyName.canonicalize(alias), "right")
        }
        for alias in ["left", "LeftArrow", "ArrowLeft"] {
            XCTAssertEqual(try KeyboardKeyName.canonicalize(alias), "left")
        }
        for alias in ["up", "UpArrow", "ArrowUp"] {
            XCTAssertEqual(try KeyboardKeyName.canonicalize(alias), "up")
        }
        for alias in ["down", "DownArrow", "ArrowDown"] {
            XCTAssertEqual(try KeyboardKeyName.canonicalize(alias), "down")
        }
    }

    func testNamedAndPrintableKeysCanonicalize() throws {
        XCTAssertEqual(try KeyboardKeyName.canonicalize("A"), "a")
        XCTAssertEqual(try KeyboardKeyName.canonicalize("Enter"), "return")
        XCTAssertEqual(try KeyboardKeyName.canonicalize("Page_Down"), "pagedown")
        XCTAssertEqual(try KeyboardKeyName.canonicalize("Forward Delete"), "forwarddelete")
        XCTAssertEqual(try KeyboardKeyName.canonicalize(";"), ";")
    }

    func testUnknownKeyIsRejected() {
        XCTAssertThrowsError(try KeyboardKeyName.canonicalize("HyperArrow")) { error in
            guard case let ComputerUseError.invalidArguments(message) = error else {
                return XCTFail("unexpected error: \(error)")
            }
            XCTAssertTrue(message.contains("HyperArrow"))
        }
    }

    func testShortcutPreservesLegacyCombinedSyntax() throws {
        XCTAssertEqual(
            try KeyboardShortcut.canonicalSpecification(key: "cmd+RightArrow", modifiers: []),
            "command+right"
        )
        XCTAssertEqual(
            try KeyboardShortcut.canonicalSpecification(key: "c", modifiers: ["command", "shift"]),
            "command+shift+c"
        )
    }
}
