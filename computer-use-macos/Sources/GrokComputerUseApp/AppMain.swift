import AppKit
import ApplicationServices
import ComputerUseCore
import CoreGraphics
import Foundation

@main
struct GrokComputerUseApplication {
    @MainActor
    static func main() {
        let application = NSApplication.shared
        application.setActivationPolicy(.accessory)
        let delegate = AppDelegate()
        application.delegate = delegate
        application.run()
        withExtendedLifetime(delegate) {}
    }
}

@MainActor
private final class AppDelegate: NSObject, NSApplicationDelegate {
    private var server: AgentSocketServer?

    func applicationDidFinishLaunching(_ notification: Notification) {
        do {
            let socketURL = try ComputerUsePaths.socketURL()
            // This must precede receipt recovery: a rejected second instance
            // must never mutate the live agent's durable action outcomes.
            let singletonLease = try AgentSocketServer.acquireSingletonLease(at: socketURL)
            requestPermissionsIfNeeded()
            let receipts = try DurableReceiptStore(directory: ComputerUsePaths.receiptsDirectory())
            let runtime = try ComputerUseRuntime(driver: MacOSDesktopDriver(), receipts: receipts)
            let server = try AgentSocketServer(
                socketURL: socketURL,
                runtime: runtime,
                peerVerifier: SignedRelayPeerVerifier(),
                singletonLease: singletonLease
            )
            self.server = server
            server.start()
        } catch {
            FileHandle.standardError.write(Data("Grok Computer Use failed to start.\n".utf8))
            NSApp.terminate(nil)
        }
    }

    func applicationWillTerminate(_ notification: Notification) {
        server?.stop()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        false
    }

    private func requestPermissionsIfNeeded() {
        if !AXIsProcessTrusted() {
            // The SDK imports kAXTrustedCheckOptionPrompt as mutable global state,
            // which Swift 6 will not access from an actor-isolated method.
            _ = AXIsProcessTrustedWithOptions(["AXTrustedCheckOptionPrompt": true] as CFDictionary)
        }
        if !CGPreflightScreenCaptureAccess() {
            _ = CGRequestScreenCaptureAccess()
        }
    }
}
