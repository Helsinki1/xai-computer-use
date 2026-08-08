package enum AgentTransportFailureDisposition: Sendable, Equatable {
    case retryReadOnlyCall
    case recoverAction(String)
    case failClosed
}

package enum AgentTransportRecoveryPolicy {
    package static func disposition(
        kind: AgentRequestKind,
        toolName: String,
        actionIdentifier: String?
    ) -> AgentTransportFailureDisposition {
        guard kind == .callTool else { return .failClosed }
        if ["list_apps", "get_app_state"].contains(toolName), actionIdentifier == nil {
            return .retryReadOnlyCall
        }
        if ToolCatalog.actionToolNames.contains(toolName), let actionIdentifier {
            return .recoverAction(actionIdentifier)
        }
        return .failClosed
    }
}
