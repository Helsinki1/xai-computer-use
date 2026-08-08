import ComputerUseCore
import Darwin
import Foundation

@main
struct GrokComputerUseRelay {
    static func main() async {
        let clientIdentifier = UUID().uuidString.lowercased()
        do {
            try TrustedHostParentVerifier.verify()
            let client = try AgentClient(clientIdentifier: clientIdentifier)
            let server = MCPServer(caller: client, clientIdentifier: clientIdentifier)
            while let line = readLine(strippingNewline: true) {
                if line.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { continue }
                if let response = await server.handle(line: line) {
                    FileHandle.standardOutput.write(Data((response + "\n").utf8))
                }
            }
            await server.disconnect()
        } catch {
            FileHandle.standardError.write(Data("Grok Computer Use MCP relay failed to initialize.\n".utf8))
            exit(EXIT_FAILURE)
        }
    }
}
