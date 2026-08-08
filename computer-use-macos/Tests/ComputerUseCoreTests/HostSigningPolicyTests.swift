import XCTest
@testable import ComputerUseCore

final class HostSigningPolicyTests: XCTestCase {
    private let relay = ProcessSigningIdentity(
        identifier: "com.xai.grok.computer-use.mcp",
        teamIdentifier: "XAI1234567",
        executableBasename: "grok-computer-use-mcp"
    )

    func testAcceptsInstalledAndRepositoryHostNames() {
        XCTAssertTrue(accepts(identifier: "grok", basename: "grok"))
        XCTAssertTrue(accepts(identifier: "xai-grok-pager", basename: "xai-grok-pager"))
    }

    func testAcceptsStrictVersionedReleaseNamesAndExplicitIdentifiers() {
        let stable = "grok-0.2.101-macos-aarch64"
        let prerelease = "grok-0.2.101-alpha.4+build.9-macos-aarch64"
        XCTAssertTrue(accepts(identifier: stable, basename: stable))
        XCTAssertTrue(accepts(identifier: "ai.x.grok", basename: stable))
        XCTAssertTrue(accepts(identifier: "com.xai.grok", basename: prerelease))
        XCTAssertTrue(accepts(identifier: "com.xai.grok.cli", basename: prerelease))
    }

    func testRejectsUnsignedMismatchedOrUnexpectedHosts() {
        XCTAssertFalse(GrokHostSigningPolicy.accepts(
            relay: ProcessSigningIdentity(
                identifier: relay.identifier,
                teamIdentifier: "",
                executableBasename: relay.executableBasename
            ),
            host: ProcessSigningIdentity(identifier: "grok", teamIdentifier: "", executableBasename: "grok")
        ))
        XCTAssertFalse(accepts(identifier: "grok", basename: "grok", teamIdentifier: "OTHERTEAM"))
        XCTAssertFalse(accepts(identifier: "grok", basename: "grok", teamIdentifier: "invalid team"))
        XCTAssertFalse(accepts(identifier: "com.xai.grok", basename: "grok-helper"))
        XCTAssertFalse(accepts(identifier: "com.xai.grok.helper", basename: "grok"))
    }

    func testRejectsMalformedVersionLikeNames() {
        let malformed = [
            "grok-1-macos-aarch64",
            "grok-1.2-macos-aarch64",
            "grok-01.2.3-macos-aarch64",
            "grok-1.02.3-macos-aarch64",
            "grok-1.2.03-macos-aarch64",
            "grok-1.2.3-01-macos-aarch64",
            "grok-1.2.3--alpha-macos-aarch64",
            "grok-1.2.3-alpha..1-macos-aarch64",
            "grok-1.2.3-linux-aarch64",
            "grok-malware-macos-aarch64",
        ]
        for name in malformed {
            XCTAssertFalse(accepts(identifier: name, basename: name), "unexpectedly accepted \(name)")
        }
    }

    func testRejectsUnexpectedRelayIdentity() {
        let host = ProcessSigningIdentity(
            identifier: "grok",
            teamIdentifier: relay.teamIdentifier,
            executableBasename: "grok"
        )
        XCTAssertFalse(GrokHostSigningPolicy.accepts(
            relay: ProcessSigningIdentity(
                identifier: "com.example.relay",
                teamIdentifier: relay.teamIdentifier,
                executableBasename: relay.executableBasename
            ),
            host: host
        ))
        XCTAssertFalse(GrokHostSigningPolicy.accepts(
            relay: ProcessSigningIdentity(
                identifier: relay.identifier,
                teamIdentifier: relay.teamIdentifier,
                executableBasename: "copied-relay"
            ),
            host: host
        ))
    }

    func testComponentPolicyRequiresExactSameTeamAppAndRelayIdentities() {
        let app = ProcessSigningIdentity(
            identifier: "com.xai.grok.computer-use",
            teamIdentifier: relay.teamIdentifier,
            executableBasename: "GrokComputerUseApp"
        )
        XCTAssertTrue(GrokComponentSigningPolicy.accepts(app: app, relay: relay))
        XCTAssertFalse(GrokComponentSigningPolicy.accepts(
            app: ProcessSigningIdentity(
                identifier: "com.example.lookalike",
                teamIdentifier: relay.teamIdentifier,
                executableBasename: app.executableBasename
            ),
            relay: relay
        ))
        XCTAssertFalse(GrokComponentSigningPolicy.accepts(
            app: app,
            relay: ProcessSigningIdentity(
                identifier: relay.identifier,
                teamIdentifier: "OTHERTEAM",
                executableBasename: relay.executableBasename
            )
        ))
    }

    func testDebugPolicyAllowsOnlyMatchingAdHocComponentIdentities() {
#if DEBUG
        let app = ProcessSigningIdentity(
            identifier: GrokComponentSigningPolicy.appIdentifier,
            teamIdentifier: "",
            executableBasename: GrokComponentSigningPolicy.appBasename
        )
        let relay = ProcessSigningIdentity(
            identifier: GrokComponentSigningPolicy.relayIdentifier,
            teamIdentifier: "",
            executableBasename: GrokComponentSigningPolicy.relayBasename
        )
        XCTAssertTrue(GrokComponentSigningPolicy.accepts(app: app, relay: relay, allowAdHoc: true))
        XCTAssertFalse(GrokHostSigningPolicy.accepts(
            relay: relay,
            host: ProcessSigningIdentity(
                identifier: "untrusted-host",
                teamIdentifier: "",
                executableBasename: "untrusted-host"
            ),
            allowAdHoc: true
        ))
#endif
    }

    private func accepts(identifier: String, basename: String, teamIdentifier: String = "XAI1234567") -> Bool {
        GrokHostSigningPolicy.accepts(
            relay: relay,
            host: ProcessSigningIdentity(
                identifier: identifier,
                teamIdentifier: teamIdentifier,
                executableBasename: basename
            )
        )
    }
}
