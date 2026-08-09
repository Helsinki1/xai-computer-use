import Foundation

public struct AccessibilityWindowCandidate: Sendable, Equatable {
    public let windowIdentifier: UInt32?
    public let frame: GlobalScreenRect?
    public let title: String?

    public init(windowIdentifier: UInt32?, frame: GlobalScreenRect?, title: String?) {
        self.windowIdentifier = windowIdentifier
        self.frame = frame
        self.title = title
    }
}

public enum AccessibilityWindowMatcher {
    public static let defaultFrameTolerancePoints = 4.0

    /// Match one accessibility window to an already PID-bound capture window.
    ///
    /// A shared native window number is authoritative. Apps that omit it are
    /// matched using tolerant geometry and normalized titles, following the
    /// recovery-oriented behavior of open-codex-computer-use. A single AX
    /// window is safe to use because the capture window was already bound to
    /// the same process.
    public static func matchIndex(
        windowIdentifier: UInt32,
        frame: GlobalScreenRect,
        title: String?,
        candidates: [AccessibilityWindowCandidate],
        frameTolerancePoints: Double = defaultFrameTolerancePoints
    ) -> Int? {
        let numbered = candidates.indices.filter {
            candidates[$0].windowIdentifier == windowIdentifier
        }
        if numbered.count == 1 {
            return numbered[0]
        }

        let pool = numbered.isEmpty ? Array(candidates.indices) : numbered
        let frameMatches = pool.filter {
            candidates[$0].frame.map {
                framesMatch($0, frame, tolerance: frameTolerancePoints)
            } == true
        }
        if frameMatches.count == 1 {
            return frameMatches[0]
        }
        if let titleMatch = uniqueTitleMatch(title, in: frameMatches, candidates: candidates) {
            return titleMatch
        }

        if pool.count == 1 {
            return pool[0]
        }
        return uniqueTitleMatch(title, in: pool, candidates: candidates)
    }

    public static func framesMatch(
        _ left: GlobalScreenRect,
        _ right: GlobalScreenRect,
        tolerance: Double = defaultFrameTolerancePoints
    ) -> Bool {
        guard tolerance.isFinite, tolerance >= 0 else { return false }
        return abs(left.x - right.x) <= tolerance
            && abs(left.y - right.y) <= tolerance
            && abs(left.width - right.width) <= tolerance
            && abs(left.height - right.height) <= tolerance
    }

    private static func uniqueTitleMatch(
        _ title: String?,
        in indices: [Int],
        candidates: [AccessibilityWindowCandidate]
    ) -> Int? {
        guard let query = normalizedTitle(title), !query.isEmpty else { return nil }
        let matches = indices.filter {
            guard let candidate = normalizedTitle(candidates[$0].title), !candidate.isEmpty else {
                return false
            }
            return candidate == query || candidate.contains(query) || query.contains(candidate)
        }
        return matches.count == 1 ? matches[0] : nil
    }

    private static func normalizedTitle(_ title: String?) -> String? {
        title?
            .folding(options: [.caseInsensitive, .diacriticInsensitive], locale: .current)
            .trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
