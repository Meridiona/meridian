// meridian — normalises screenpipe activity into structured app sessions
//
// Reads the complete text content of whatever VS Code panel is currently
// focused — editor, terminal, output, search, etc. — using the macOS
// Accessibility API directly.
//
// Screenpipe's AX walker caps at depth 30; the VS Code terminal sits at
// depth 30 (excluded by >= check). This script walks to depth 35 and
// also handles the terminal scrollback buffer via the accessible view.
//
// Usage:
//   swift swift/vscode_panel_reader.swift              → JSON to stdout
//   swift swift/vscode_panel_reader.swift --text       → plain text to stdout
//   swift swift/vscode_panel_reader.swift --watch 5    → poll every 5 seconds
//
// Compile for faster repeated use:
//   swiftc swift/vscode_panel_reader.swift -o /usr/local/bin/vscode-panel-reader -O

import AppKit
import ApplicationServices

// ---------------------------------------------------------------------------
// AX helpers
// ---------------------------------------------------------------------------

func axAttr<T>(_ el: AXUIElement, _ attr: String) -> T? {
    var ref: CFTypeRef?
    guard AXUIElementCopyAttributeValue(el, attr as CFString, &ref) == .success else { return nil }
    return ref as? T
}

func axParent(_ el: AXUIElement) -> AXUIElement? {
    return axAttr(el, kAXParentAttribute as String)
}

func axChildren(_ el: AXUIElement) -> [AXUIElement] {
    return axAttr(el, kAXChildrenAttribute as String) ?? []
}

func axRole(_ el: AXUIElement) -> String {
    return axAttr(el, kAXRoleAttribute as String) ?? ""
}

func axDesc(_ el: AXUIElement) -> String {
    return axAttr(el, kAXDescriptionAttribute as String) ?? ""
}

func axValue(_ el: AXUIElement) -> String {
    return axAttr(el, kAXValueAttribute as String) ?? ""
}

func axTitle(_ el: AXUIElement) -> String {
    return axAttr(el, kAXTitleAttribute as String) ?? ""
}

// ---------------------------------------------------------------------------
// Tree walk — collects non-empty text nodes up to maxDepth
// ---------------------------------------------------------------------------

struct TextNode {
    let role: String
    let desc: String
    let value: String
    let depth: Int
}

func walk(_ el: AXUIElement, depth: Int, maxDepth: Int, deadline: Date, into results: inout [TextNode]) {
    guard depth <= maxDepth, Date() < deadline else { return }

    let role  = axRole(el)
    let desc  = axDesc(el)
    let value = axValue(el)

    let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
    if !trimmed.isEmpty {
        results.append(TextNode(role: role, desc: desc, value: value, depth: depth))
    }

    for child in axChildren(el) {
        walk(child, depth: depth + 1, maxDepth: maxDepth, deadline: deadline, into: &results)
    }
}

// ---------------------------------------------------------------------------
// Terminal scrollback via VS Code Accessible View (⌥F2)
// Non-disruptive: opens, reads, closes in ~300ms.
// ---------------------------------------------------------------------------

func runOsascript(_ script: String) {
    let task = Process()
    task.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
    task.arguments = ["-e", script]
    try? task.run()
    task.waitUntilExit()
}

func readTerminalViaAccessibleView(pid: pid_t) -> String? {
    // Bring VS Code to front first, then send ⌥F2 via System Events
    // (CGEvent.postToPid unreliable when VS Code isn't the frontmost app)
    runOsascript("tell application \"Code\" to activate")
    Thread.sleep(forTimeInterval: 0.2)
    runOsascript("tell application \"System Events\" to tell application process \"Code\" to key code 120 using option down")
    Thread.sleep(forTimeInterval: 0.7)

    // After ⌥F2, VS Code moves focus to the Accessible View AXTextArea.
    // Read it via AXFocusedUIElement rather than BFS.
    let app: AXUIElement = AXUIElementCreateApplication(pid)
    var found: String?

    let deadline = Date(timeIntervalSinceNow: 2.0)
    while Date() < deadline {
        if let focused: AXUIElement = axAttr(app, kAXFocusedUIElementAttribute as String) {
            let r = axRole(focused)
            let d = axDesc(focused)
            if r == "AXTextArea" && d.contains("Accessible View") {
                found = axValue(focused)
                break
            }
        }
        Thread.sleep(forTimeInterval: 0.1)
    }

    // Escape to close accessible view
    runOsascript("tell application \"System Events\" to tell application process \"Code\" to key code 53")

    return found
}

// ---------------------------------------------------------------------------
// Main capture logic
// ---------------------------------------------------------------------------

struct CaptureResult: Encodable {
    let panelKind: String     // "terminal" | "editor" | "other"
    let role: String
    let description: String
    let text: String
    let charCount: Int
    let capturedAt: String
}

func capture() -> CaptureResult {
    let apps = NSWorkspace.shared.runningApplications
    guard let vscode = apps.first(where: {
        $0.bundleIdentifier == "com.microsoft.VSCode" || $0.localizedName == "Code"
    }) else {
        return CaptureResult(panelKind: "error", role: "", description: "",
                             text: "VS Code not running", charCount: 0,
                             capturedAt: ISO8601DateFormatter().string(from: Date()))
    }

    let pid = vscode.processIdentifier
    let app: AXUIElement = AXUIElementCreateApplication(pid)

    // Ensure Electron exposes its a11y tree
    AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
    AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)

    guard let focused: AXUIElement = axAttr(app, kAXFocusedUIElementAttribute as String) else {
        return CaptureResult(panelKind: "error", role: "", description: "",
                             text: "No focused element in VS Code", charCount: 0,
                             capturedAt: ISO8601DateFormatter().string(from: Date()))
    }

    let role = axRole(focused)
    let desc = axDesc(focused)
    let directValue = axValue(focused)

    // Determine panel kind from description / role
    let isTerminal = desc.lowercased().contains("terminal") || axTitle(focused).lowercased().contains("terminal")
    let panelKind: String

    var text: String

    if isTerminal {
        panelKind = "terminal"
        if !directValue.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            // Terminal input field has content (user is typing)
            text = directValue
        } else {
            // Terminal is running a command — use accessible view for full scrollback
            text = readTerminalViaAccessibleView(pid: pid) ?? ""
        }
    } else {
        // Editor or other panel: walk the focused window to depth 35
        // Go up from focused element to find the nearest panel/group container
        panelKind = (role == "AXTextArea") ? "editor" : "other"

        // Find container ~5 levels up
        var container = focused
        for _ in 0..<4 {
            if let p = axParent(container) { container = p } else { break }
        }

        // Walk from that container, depth 12 relative (= ~depth 35 from root)
        var nodes: [TextNode] = []
        let deadline = Date(timeIntervalSinceNow: 3.0)
        walk(container, depth: 0, maxDepth: 12, deadline: deadline, into: &nodes)

        // Prefer the largest text node — that's the active editor buffer
        let sorted = nodes.sorted { $0.value.count > $1.value.count }
        text = sorted.first?.value ?? directValue
    }

    let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
    return CaptureResult(
        panelKind: panelKind,
        role: role,
        description: desc,
        text: trimmed,
        charCount: trimmed.count,
        capturedAt: ISO8601DateFormatter().string(from: Date())
    )
}

// ---------------------------------------------------------------------------
// CLI entry point
// ---------------------------------------------------------------------------

let args = CommandLine.arguments
let plainText = args.contains("--text")
let watchIndex = args.firstIndex(of: "--watch")
let watchInterval = watchIndex.flatMap { args.indices.contains($0 + 1) ? Double(args[$0 + 1]) : nil } ?? 0

func output(_ result: CaptureResult) {
    if plainText {
        print("=== VS Code Panel [\(result.panelKind)] @ \(result.capturedAt) ===")
        print(result.text)
    } else {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        if let data = try? encoder.encode(result), let str = String(data: data, encoding: .utf8) {
            print(str)
        }
    }
}

if watchInterval > 0 {
    while true {
        output(capture())
        Thread.sleep(forTimeInterval: watchInterval)
    }
} else {
    output(capture())
}
