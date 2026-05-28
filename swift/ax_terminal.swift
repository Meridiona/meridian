// meridian — normalises screenpipe activity into structured app sessions
import AppKit
import ApplicationServices

func ax<T>(_ e: AXUIElement, _ a: String) -> T? {
    var r: CFTypeRef?
    guard AXUIElementCopyAttributeValue(e, a as CFString, &r) == .success else { return nil }
    return r as? T
}

guard let vscode = NSWorkspace.shared.runningApplications.first(where: { $0.localizedName == "Code" }) else {
    fputs("error: VS Code not running\n", stderr)
    exit(1)
}
let app = AXUIElementCreateApplication(vscode.processIdentifier)
// Per-element IPC timeout — prevents hanging when called from launchd/daemon context
AXUIElementSetMessagingTimeout(app, 2.0)
AXUIElementSetAttributeValue(app, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
AXUIElementSetAttributeValue(app, "AXManualAccessibility" as CFString, kCFBooleanTrue)

guard let focused: AXUIElement = ax(app, "AXFocusedUIElement") else {
    fputs("error: no focused element in VS Code\n", stderr)
    exit(1)
}
var cur: AXUIElement = focused
var webArea: AXUIElement?
while let p: AXUIElement = ax(cur, "AXParent") {
    if ax(p, "AXRole") as String? == "AXWebArea" { webArea = p; break }
    cur = p
}

guard let webArea = webArea else {
    fputs("error: AXWebArea not found\n", stderr)
    exit(1)
}

let deadline = Date(timeIntervalSinceNow: 5)
func walk(_ el: AXUIElement, _ d: Int) {
    guard d <= 30, Date() < deadline else { return }
    let v: String = ax(el, "AXValue") ?? ""
    if !v.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { print(v) }
    for k: AXUIElement in ax(el, "AXChildren") ?? [] { walk(k, d + 1) }
}
walk(webArea, 0)
