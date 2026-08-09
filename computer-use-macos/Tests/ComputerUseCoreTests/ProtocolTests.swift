import Foundation
import XCTest
@testable import ComputerUseCore

final class ProtocolTests: XCTestCase {
    func testCatalogHasExactNineCanonicalSchemas() throws {
        let expectedData = Data(canonicalSchemas.utf8)
        let expected = try JSONDecoder().decode([String: JSONValue].self, from: expectedData)
        XCTAssertEqual(Set(ToolCatalog.all.map(\.name)), Set(expected.keys))
        XCTAssertEqual(ToolCatalog.all.count, 9)
        for tool in ToolCatalog.all {
            XCTAssertEqual(tool.inputSchema, expected[tool.name], "schema drift for \(tool.name)")
        }
    }

    func testClickGuidancePrefersCurrentSemanticTargetsAndSafeRecovery() throws {
        let getState = try XCTUnwrap(ToolCatalog.all.first(where: { $0.name == "get_app_state" }))
        let click = try XCTUnwrap(ToolCatalog.all.first(where: { $0.name == "click" }))

        XCTAssertTrue(getState.description.contains("target_candidates"))
        XCTAssertTrue(click.description.contains("Prefer target.kind=element"))
        XCTAssertTrue(click.description.contains("do not replay an uncertain action"))
        XCTAssertTrue(click.description.contains("System Settings"))
    }

    func testSystemSettingsIsBlockedByBundleAndExplicitLaunchControl() throws {
        XCTAssertTrue(AppAccessPolicy.isBlocked(bundleIdentifier: "com.apple.systempreferences"))
        XCTAssertTrue(AppAccessPolicy.isBlocked(bundleIdentifier: "COM.APPLE.SYSTEMPREFERENCES"))
        XCTAssertFalse(AppAccessPolicy.isBlocked(bundleIdentifier: "com.example.fixture"))
        XCTAssertThrowsError(try AppAccessPolicy.requireAllowed(bundleIdentifier: "com.apple.systempreferences"))

        let launchControl = AccessibilityElementSnapshot(
            identifier: "e1",
            role: "AXButton",
            label: "Open System Settings",
            value: nil,
            frame: nil,
            actions: ["AXPress"],
            isValueSettable: false,
            driverToken: "private"
        )
        XCTAssertTrue(AppAccessPolicy.isSystemSettingsLaunchControl(launchControl))
    }

    func testAuthenticatedAgentEnvelopeDetectsMutation() throws {
        let key = Data(repeating: 7, count: 32)
        let base = AgentRequest(
            requestIdentifier: UUID().uuidString,
            clientIdentifier: UUID().uuidString,
            kind: .callTool,
            toolName: "list_apps",
            arguments: [:]
        )
        let request = try base.authenticated(sessionIdentifier: UUID().uuidString, sequence: 1, key: key)
        try request.validate()
        XCTAssertTrue(AgentSessionAuthentication.verify(
            tag: try XCTUnwrap(request.authenticationTag),
            key: key,
            domain: "request-v2",
            payload: try request.authenticationPayload()
        ))

        let mutated = AgentRequest(
            requestIdentifier: request.requestIdentifier,
            clientIdentifier: request.clientIdentifier,
            kind: .callTool,
            toolName: "list_apps",
            arguments: ["unexpected": .bool(true)],
            sessionIdentifier: request.sessionIdentifier,
            sequence: request.sequence,
            authenticationTag: request.authenticationTag
        )
        XCTAssertFalse(AgentSessionAuthentication.verify(
            tag: try XCTUnwrap(mutated.authenticationTag),
            key: key,
            domain: "request-v2",
            payload: try mutated.authenticationPayload()
        ))
    }

    func testAgentWireRejectsUnknownFields() throws {
        let request = """
        {"version":2,"requestIdentifier":"\(UUID().uuidString)","clientIdentifier":"\(UUID().uuidString)","kind":"initialize","sessionSecret":"\(Data(repeating: 1, count: 32).base64EncodedString())","forged":true}
        """
        XCTAssertThrowsError(try AgentWire.decodeRequest(Data(request.utf8)))
    }

    func testProtectedCarrierHasTheExactCrossLanguageShape() throws {
        let snapshotID = "00000000-0000-4000-8000-000000000001"
        let attestationID = "00000000-0000-4000-8000-000000000002"
        let hash = String(repeating: "a", count: 64)
        let captured = CapturedDesktopState(
            app: AppTarget(name: "Fixture", bundleIdentifier: "com.example.fixture", processIdentifier: 42),
            windowTitle: "Fixture",
            geometry: WindowGeometry(
                windowIdentifier: 17,
                globalBoundsPoints: GlobalScreenRect(x: -20, y: 40, width: 640, height: 480),
                pngWidthPixels: 1_280,
                pngHeightPixels: 960
            ),
            screenshotPNG: Data([0x89, 0x50, 0x4e, 0x47]),
            screenshotSHA256: hash,
            accessibilityTree: "",
            elements: []
        )
        let carrier = try ProtectedComputerUseCarrier(snapshot: SnapshotEnvelope(
            snapshotIdentifier: snapshotID,
            deliveryAttestationIdentifier: attestationID,
            captured: captured,
            leaseExpiresAt: Date(timeIntervalSince1970: 1_800_000_000)
        ))

        XCTAssertEqual(carrier.json, .object([
            "profile": .string("computer-use-v2"),
            "snapshot_id": .string(snapshotID),
            "attestation_id": .string(attestationID),
            "bundle_id": .string("com.example.fixture"),
            "window_id": .integer(17),
            "png_sha256": .string(hash),
            "png_width_px": .integer(1_280),
            "png_height_px": .integer(960),
            "capture_origin_x": .number(-20),
            "capture_origin_y": .number(40),
            "capture_width_points": .number(640),
            "capture_height_points": .number(480),
        ]))
    }

    func testTransportRecoveryNeverRetriesLifecycleCalls() {
        for kind in [
            AgentRequestKind.attestSnapshotDelivery,
            .invalidateSession,
            .leaseHeartbeat,
            .releaseOperation,
        ] {
            XCTAssertEqual(
                AgentTransportRecoveryPolicy.disposition(
                    kind: kind,
                    toolName: kind.rawValue,
                    actionIdentifier: nil
                ),
                .failClosed
            )
        }
    }

    func testTransportRecoverySeparatesReadsFromActions() {
        XCTAssertEqual(
            AgentTransportRecoveryPolicy.disposition(
                kind: .callTool,
                toolName: "get_app_state",
                actionIdentifier: nil
            ),
            .retryReadOnlyCall
        )
        XCTAssertEqual(
            AgentTransportRecoveryPolicy.disposition(
                kind: .callTool,
                toolName: "click",
                actionIdentifier: "action-1"
            ),
            .recoverAction("action-1")
        )
        XCTAssertEqual(
            AgentTransportRecoveryPolicy.disposition(
                kind: .callTool,
                toolName: "click",
                actionIdentifier: nil
            ),
            .failClosed
        )
    }

    func testListAppsAcceptsNullAsTheOptionalEmptyArgumentsObject() async throws {
        let caller = RecordingToolCaller()
        let server = MCPServer(caller: caller, clientIdentifier: "test-client")
        _ = await server.handle(line: #"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{}}}"#)
        let rawResponse = await server.handle(line: #"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_apps","arguments":null,"_meta":{"xai/computer-use-v2":{"profile":"computer-use-v2","logical_call_id":"logical-call","session_id":"session","workflow_id":"workflow","action_id":"action","tool_name":"list_apps"},"progressToken":1}}}"#)
        let response = try XCTUnwrap(rawResponse)

        let decoded = try JSONDecoder().decode(JSONValue.self, from: Data(response.utf8))
        XCTAssertNil(decoded.objectValue?["error"])
        let call = await caller.lastCall
        XCTAssertEqual(call?.name, "list_apps")
        XCTAssertEqual(call?.arguments, [:])
    }
}

private actor RecordingToolCaller: ToolCalling {
    private(set) var lastCall: (name: String, arguments: [String: JSONValue])?

    func callTool(
        name: String,
        arguments: [String: JSONValue],
        context: ToolCallContext
    ) async -> ToolExecutionResult {
        lastCall = (name, arguments)
        return ToolExecutionResult(text: "ok")
    }

    func actionOutcome(identifier: String) async -> ActionReceipt? { nil }
    func clientDisconnected(identifier: String) async {}
}

private let canonicalSchemas = #"""
{
  "list_apps":{"type":"object","properties":{},"additionalProperties":false},
  "get_app_state":{"type":"object","properties":{"bundle_id":{"type":"string","minLength":3,"maxLength":255},"window_id":{"type":"integer","minimum":1,"maximum":4294967295}},"additionalProperties":false,"required":["bundle_id"]},
  "click":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"target":{"oneOf":[{"type":"object","properties":{"kind":{"const":"element"},"element_id":{"type":"string","minLength":1,"maxLength":128}},"additionalProperties":false,"required":["kind","element_id"]},{"type":"object","properties":{"kind":{"const":"pixel"},"x_px":{"type":"number"},"y_px":{"type":"number"}},"additionalProperties":false,"required":["kind","x_px","y_px"]}]},"button":{"type":"string","enum":["left","right"]},"count":{"type":"integer","enum":[1,2]}},"additionalProperties":false,"required":["snapshot_id","target"]},
  "drag":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"from":{"type":"object","properties":{"x_px":{"type":"number"},"y_px":{"type":"number"}},"additionalProperties":false,"required":["x_px","y_px"]},"to":{"type":"object","properties":{"x_px":{"type":"number"},"y_px":{"type":"number"}},"additionalProperties":false,"required":["x_px","y_px"]}},"additionalProperties":false,"required":["snapshot_id","from","to"]},
  "perform_secondary_action":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"element_id":{"type":"string","minLength":1,"maxLength":128},"action_id":{"type":"string","minLength":1,"maxLength":128}},"additionalProperties":false,"required":["snapshot_id","element_id","action_id"]},
  "scroll":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"element_id":{"type":"string","minLength":1,"maxLength":128},"direction":{"type":"string","enum":["up","down","left","right"]},"pages":{"type":"number","exclusiveMinimum":0,"maximum":10}},"additionalProperties":false,"required":["snapshot_id","element_id","direction"]},
  "type_text":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"text":{"type":"string","maxLength":32768}},"additionalProperties":false,"required":["snapshot_id","text"]},
  "press_key":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"key":{"type":"string"},"modifiers":{"type":"array","items":{"type":"string","enum":["command","control","option","shift","fn"]},"uniqueItems":true}},"additionalProperties":false,"required":["snapshot_id","key"]},
  "set_value":{"type":"object","properties":{"snapshot_id":{"type":"string","minLength":16,"maxLength":128},"element_id":{"type":"string","minLength":1,"maxLength":128},"value":{"type":"string","maxLength":32768}},"additionalProperties":false,"required":["snapshot_id","element_id","value"]}
}
"""#
