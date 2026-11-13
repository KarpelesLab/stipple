// An out-of-process accessibility client for macOS CI: it reads a running
// app's NSAccessibility tree through the AX API (AXUIElementCreateApplication,
// AXUIElementCopyAttributeValue), the same calls VoiceOver and other
// assistive apps make from their own process.
//
//   axprobe <pid> [button label to press]
//
// It prints "axprobe: trusted=<bool>" (AXIsProcessTrusted: whether TCC lets
// this process use the AX API), then every element under the app's windows
// with its role, description, title, value and frame (screen coordinates, top
// left origin), then the focused element, and finally tries AXPress on the
// named button and reads the tree again. Exit status: 0 when the tree was
// read, 3 when the AX API is refused (no accessibility permission), 4 when the
// app shows no window to AX.
import ApplicationServices
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)

guard CommandLine.arguments.count >= 2, let pid = pid_t(CommandLine.arguments[1]) else {
    print("usage: axprobe <pid> [button label to press]")
    exit(2)
}
let press = CommandLine.arguments.count >= 3 ? CommandLine.arguments[2] : nil

print("axprobe: trusted=\(AXIsProcessTrusted())")

func copy(_ e: AXUIElement, _ name: String) -> (AXError, CFTypeRef?) {
    var v: CFTypeRef?
    let err = AXUIElementCopyAttributeValue(e, name as CFString, &v)
    return (err, v)
}

func string(_ e: AXUIElement, _ name: String) -> String? {
    let (err, v) = copy(e, name)
    guard err == .success, let v else { return nil }
    if let s = v as? String { return s }
    if let n = v as? NSNumber { return n.stringValue }
    return nil
}

func frame(_ e: AXUIElement) -> CGRect? {
    let (e1, p) = copy(e, kAXPositionAttribute)
    let (e2, s) = copy(e, kAXSizeAttribute)
    guard e1 == .success, e2 == .success, let p, let s else { return nil }
    var point = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(p as! AXValue, .cgPoint, &point),
          AXValueGetValue(s as! AXValue, .cgSize, &size) else { return nil }
    return CGRect(origin: point, size: size)
}

func children(_ e: AXUIElement) -> [AXUIElement] {
    let (err, v) = copy(e, kAXChildrenAttribute)
    guard err == .success, let list = v as? [AXUIElement] else { return [] }
    return list
}

func describe(_ e: AXUIElement) -> String {
    var parts = ["role=\(string(e, kAXRoleAttribute) ?? "?")"]
    for (key, name) in [("desc", kAXDescriptionAttribute), ("title", kAXTitleAttribute),
                        ("value", kAXValueAttribute)] {
        if let s = string(e, name), !s.isEmpty { parts.append("\(key)=\"\(s)\"") }
    }
    if let f = frame(e) {
        parts.append("frame=\(Int(f.minX)),\(Int(f.minY)),\(Int(f.width))x\(Int(f.height))")
    }
    return parts.joined(separator: " ")
}

struct Found {
    var count = 0
    var roles: [String: Int] = [:]
    var buttons: [String: AXUIElement] = [:]
}

func walk(_ e: AXUIElement, _ depth: Int, _ found: inout Found) {
    found.count += 1
    let role = string(e, kAXRoleAttribute) ?? "?"
    found.roles[role, default: 0] += 1
    if role == "AXButton", let d = string(e, kAXDescriptionAttribute) ?? string(e, kAXTitleAttribute) {
        found.buttons[d] = found.buttons[d] ?? e
    }
    print("axprobe: \(String(repeating: "  ", count: depth))\(describe(e))")
    if depth < 40 {
        for c in children(e) { walk(c, depth + 1, &found) }
    }
}

let app = AXUIElementCreateApplication(pid)
// Ask for the windows a few times: the app may still be starting.
var windows: [AXUIElement] = []
var lastError = AXError.success
for _ in 0..<20 {
    let (err, v) = copy(app, kAXWindowsAttribute)
    lastError = err
    if err == .success, let list = v as? [AXUIElement], !list.isEmpty {
        windows = list
        break
    }
    if err == .apiDisabled { break }
    usleep(500_000)
}
if lastError == .apiDisabled {
    print("axprobe: AX API refused (kAXErrorAPIDisabled): this process has no accessibility permission")
    exit(3)
}
if windows.isEmpty {
    print("axprobe: no windows (AXError \(lastError.rawValue))")
    exit(4)
}

func readTree() -> (Found, [CGRect]) {
    var found = Found()
    var frames: [CGRect] = []
    for w in windows {
        if let f = frame(w) { frames.append(f) }
        walk(w, 0, &found)
    }
    return (found, frames)
}

let (found, windowFrames) = readTree()
let roles = found.roles.sorted { $0.key < $1.key }.map { "\($0.key)=\($0.value)" }.joined(separator: " ")
print("axprobe: windows=\(windows.count) elements=\(found.count) \(roles)")

// Whether the elements' frames are where the window is on screen.
for (label, e) in found.buttons.sorted(by: { $0.key < $1.key }).prefix(3) {
    if let f = frame(e) {
        let inside = windowFrames.contains { $0.insetBy(dx: -1, dy: -1).contains(f) }
        print("axprobe: button \"\(label)\" frame inside its window=\(inside)")
    }
}

let (ferr, focused) = copy(app, kAXFocusedUIElementAttribute)
if ferr == .success, let focused {
    print("axprobe: focused \(describe(focused as! AXUIElement))")
} else {
    print("axprobe: focused none (AXError \(ferr.rawValue))")
}

if let press, let button = found.buttons[press] {
    var names: CFArray?
    AXUIElementCopyActionNames(button, &names)
    print("axprobe: actions of \"\(press)\": \((names as? [String]) ?? [])")
    let err = AXUIElementPerformAction(button, kAXPressAction as CFString)
    print("axprobe: AXPress \"\(press)\" -> AXError \(err.rawValue)")
    usleep(1_000_000)
    let (after, _) = readTree()
    print("axprobe: after the press: elements=\(after.count)")
}
print("axprobe: done")
