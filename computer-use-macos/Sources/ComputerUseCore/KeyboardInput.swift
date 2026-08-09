import Foundation

public enum KeyboardKeyName {
    /// Convert model- and platform-style key names to the canonical names used
    /// by the macOS driver. This validation runs before a durable action is
    /// marked dispatched, so unsupported names cannot become uncertain input.
    public static func canonicalize(_ value: String) throws -> String {
        let key = value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()

        if key.utf8.count == 1, let byte = key.utf8.first,
           (byte >= 97 && byte <= 122)
            || (byte >= 48 && byte <= 57)
            || "=[]-';\\,/.".utf8.contains(byte)
        {
            return key
        }

        if let canonical = aliases[key] {
            return canonical
        }
        throw ComputerUseError.invalidArguments("Unsupported key name: \(value).")
    }

    private static let aliases: [String: String] = [
        "return": "return",
        "enter": "return",
        "tab": "tab",
        "space": "space",
        "spacebar": "space",
        "backspace": "backspace",
        "delete": "backspace",
        "escape": "escape",
        "esc": "escape",
        "left": "left",
        "leftarrow": "left",
        "left arrow": "left",
        "left-arrow": "left",
        "left_arrow": "left",
        "arrowleft": "left",
        "arrow-left": "left",
        "arrow_left": "left",
        "right": "right",
        "rightarrow": "right",
        "right arrow": "right",
        "right-arrow": "right",
        "right_arrow": "right",
        "arrowright": "right",
        "arrow-right": "right",
        "arrow_right": "right",
        "up": "up",
        "uparrow": "up",
        "up arrow": "up",
        "up-arrow": "up",
        "up_arrow": "up",
        "arrowup": "up",
        "arrow-up": "up",
        "arrow_up": "up",
        "down": "down",
        "downarrow": "down",
        "down arrow": "down",
        "down-arrow": "down",
        "down_arrow": "down",
        "arrowdown": "down",
        "arrow-down": "down",
        "arrow_down": "down",
        "home": "home",
        "end": "end",
        "pageup": "pageup",
        "page up": "pageup",
        "page-up": "pageup",
        "page_up": "pageup",
        "pagedown": "pagedown",
        "page down": "pagedown",
        "page-down": "pagedown",
        "page_down": "pagedown",
        "forwarddelete": "forwarddelete",
        "forward delete": "forwarddelete",
        "forward-delete": "forwarddelete",
        "forward_delete": "forwarddelete",
        "deleteforward": "forwarddelete",
    ]
}

public enum KeyboardShortcut {
    /// Validate both the preferred separate modifier field and legacy key
    /// strings such as `cmd+c`, returning one canonical driver specification.
    public static func canonicalSpecification(key: String, modifiers: [String]) throws -> String {
        let components = key.split(separator: "+", omittingEmptySubsequences: false).map(String.init)
        guard let rawKey = components.last, !rawKey.isEmpty else {
            throw ComputerUseError.invalidArguments("Invalid key specification.")
        }

        var canonicalModifiers: [String] = []
        var seen = Set<String>()
        for rawModifier in modifiers + Array(components.dropLast()) {
            let modifier = try canonicalModifier(rawModifier)
            if seen.insert(modifier).inserted {
                canonicalModifiers.append(modifier)
            }
        }
        return (canonicalModifiers + [try KeyboardKeyName.canonicalize(rawKey)]).joined(separator: "+")
    }

    private static func canonicalModifier(_ value: String) throws -> String {
        switch value.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "cmd", "command", "meta", "super": "command"
        case "ctrl", "control": "control"
        case "alt", "option": "option"
        case "shift": "shift"
        case "fn": "fn"
        default: throw ComputerUseError.invalidArguments("Unsupported key modifier: \(value).")
        }
    }
}
