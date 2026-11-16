// An out-of-process accessibility client for macOS CI: it reads a running
// app's NSAccessibility tree through the AX API (AXUIElementCreateApplication,
// AXUIElementCopyAttributeValue), the same calls VoiceOver and other
// assistive apps make from their own process.
//
//   axprobe <pid> [button label to press]
//   axprobe <pid> --depth
//
// It prints "axprobe: trusted=<bool>" (AXIsProcessTrusted: whether TCC lets
// this process use the AX API), then every element under the app's windows
// with its role, description, title, value and frame (screen coordinates, top
// left origin), then the focused element, and finally tries AXPress on the
// named button and reads the tree again. Exit status: 0 when the tree was
// read, 3 when the AX API is refused (no accessibility permission), 4 when the
// app shows no window to AX.
//
// With --depth it checks the values, text ranges, checkbox, scroll area and
// actions instead (see runDepth at the end), printing "axprobe: depth ..."
// lines for the CI step to check.
import ApplicationServices
import Foundation

setvbuf(stdout, nil, _IOLBF, 0)

guard CommandLine.arguments.count >= 2, let pid = pid_t(CommandLine.arguments[1]) else {
    print("usage: axprobe <pid> [button label to press]")
    exit(2)
}
let depthMode = CommandLine.arguments.count >= 3 && CommandLine.arguments[2] == "--depth"
let press = CommandLine.arguments.count >= 3 && !depthMode ? CommandLine.arguments[2] : nil

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
if depthMode {
    runDepth()
    print("axprobe: done")
    exit(0)
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

// ---- --depth --------------------------------------------------------------

// Every element under the windows: (role, description or title, element).
// The app builds its elements again after each change, so this is read
// afresh after every action.
func allElements() -> [(String, String, AXUIElement)] {
    var out: [(String, String, AXUIElement)] = []
    func go(_ e: AXUIElement, _ depth: Int) {
        let role = string(e, kAXRoleAttribute) ?? "?"
        let name = string(e, kAXDescriptionAttribute) ?? string(e, kAXTitleAttribute) ?? ""
        out.append((role, name, e))
        if depth < 40 {
            for c in children(e) { go(c, depth + 1) }
        }
    }
    for w in windows { go(w, 0) }
    return out
}

func find(_ role: String, _ name: String? = nil) -> AXUIElement? {
    return allElements().first { $0.0 == role && (name == nil || $0.1 == name!) }?.2
}

// Let the app take what was asked (its event loop delivers it) and deliver
// any notifications to the observer.
func settle() {
    CFRunLoopRunInMode(CFRunLoopMode.defaultMode, 1.0, false)
}

func settable(_ e: AXUIElement, _ name: String) -> Bool {
    var ok: DarwinBoolean = false
    AXUIElementIsAttributeSettable(e, name as CFString, &ok)
    return ok.boolValue
}

func range(_ e: AXUIElement, _ name: String) -> String {
    let (err, v) = copy(e, name)
    var r = CFRange()
    guard err == .success, let v, AXValueGetValue(v as! AXValue, .cfRange, &r) else {
        return "none"
    }
    return "\(r.location),\(r.length)"
}

func actions(_ e: AXUIElement) -> [String] {
    var names: CFArray?
    AXUIElementCopyActionNames(e, &names)
    return (names as? [String]) ?? []
}

func runDepth() {
    // Value changes, announcements and layout changes the app posts.
    let callback: AXObserverCallback = { _, _, name, _ in
        print("axprobe: depth notification \(name as String)")
    }
    var observer: AXObserver?
    if AXObserverCreate(pid, callback, &observer) == .success, let observer {
        for n in [kAXValueChangedNotification, kAXSelectedTextChangedNotification,
                  "AXAnnouncementRequested", kAXLayoutChangedNotification] {
            let err = AXObserverAddNotification(observer, app, n as CFString, nil)
            print("axprobe: depth observing \(n) -> AXError \(err.rawValue)")
        }
        CFRunLoopAddSource(CFRunLoopGetCurrent(), AXObserverGetRunLoopSource(observer),
                           CFRunLoopMode.defaultMode)
    }

    // The text field: its value, length, and setting both from here.
    if let field = find("AXTextField") {
        print("axprobe: depth field value=\"\(string(field, kAXValueAttribute) ?? "")\" "
              + "characters=\(string(field, kAXNumberOfCharactersAttribute) ?? "?") "
              + "settable=\(settable(field, kAXValueAttribute))")
        let err = AXUIElementSetAttributeValue(field, kAXValueAttribute as CFString, "hi" as CFString)
        print("axprobe: depth set field value -> AXError \(err.rawValue)")
        settle()
        settle()
        if let field = find("AXTextField") {
            print("axprobe: depth field value after=\"\(string(field, kAXValueAttribute) ?? "")\" "
                  + "characters=\(string(field, kAXNumberOfCharactersAttribute) ?? "?") "
                  + "selected=\(range(field, kAXSelectedTextRangeAttribute))")
            var r = CFRange(location: 0, length: 1)
            if let v = AXValueCreate(.cfRange, &r) {
                let err = AXUIElementSetAttributeValue(
                    field, kAXSelectedTextRangeAttribute as CFString, v)
                print("axprobe: depth set selected range -> AXError \(err.rawValue)")
            }
            settle()
            if let field = find("AXTextField") {
                print("axprobe: depth field selected after=\(range(field, kAXSelectedTextRangeAttribute)) "
                      + "text=\"\(string(field, kAXSelectedTextAttribute) ?? "")\"")
            }
        }
    } else {
        print("axprobe: depth no AXTextField")
    }

    // The checkbox: its value, pressed, its value again.
    if let box = find("AXCheckBox", "Remember") {
        print("axprobe: depth checkbox value=\(string(box, kAXValueAttribute) ?? "none") "
              + "actions=\(actions(box))")
        let err = AXUIElementPerformAction(box, kAXPressAction as CFString)
        print("axprobe: depth press checkbox -> AXError \(err.rawValue)")
        settle()
        if let box = find("AXCheckBox", "Remember") {
            print("axprobe: depth checkbox value after=\(string(box, kAXValueAttribute) ?? "none")")
        }
    } else {
        print("axprobe: depth no AXCheckBox \"Remember\"")
    }

    // "Row 2" raises a toast, which the app asks VoiceOver to announce.
    if let row = find("AXButton", "Row 2") {
        let err = AXUIElementPerformAction(row, kAXPressAction as CFString)
        print("axprobe: depth press Row 2 -> AXError \(err.rawValue)")
        settle()
    }

    // The list: a scroll area with a vertical scroll bar.
    let areas = allElements().filter { $0.0 == "AXScrollArea" }.map { $0.2 }
    print("axprobe: depth scroll areas=\(areas.count)")
    if let area = areas.first {
        var bar: AXUIElement?
        let (err, v) = copy(area, kAXVerticalScrollBarAttribute)
        if err == .success, let v { bar = (v as! AXUIElement) }
        if bar == nil {
            bar = children(area).first { string($0, kAXRoleAttribute) == "AXScrollBar" }
        }
        if let bar {
            print("axprobe: depth scroll bar value=\(string(bar, kAXValueAttribute) ?? "none") "
                  + "settable=\(settable(bar, kAXValueAttribute)) actions=\(actions(bar))")
            let err = AXUIElementSetAttributeValue(
                bar, kAXValueAttribute as CFString, NSNumber(value: 0.5) as CFNumber)
            print("axprobe: depth set scroll bar -> AXError \(err.rawValue)")
            settle()
            if let area = allElements().first(where: { $0.0 == "AXScrollArea" })?.2,
               let bar = children(area).first(where: { string($0, kAXRoleAttribute) == "AXScrollBar" }) {
                print("axprobe: depth scroll bar value after=\(string(bar, kAXValueAttribute) ?? "none")")
            }
        } else {
            print("axprobe: depth no scroll bar")
        }
    }

    // A row out of view: AXScrollToVisible brings it in.
    if let row = find("AXButton", "Row 0") {
        let names = actions(row)
        print("axprobe: depth actions of \"Row 0\": \(names)")
        if names.contains("AXScrollToVisible") {
            let err = AXUIElementPerformAction(row, "AXScrollToVisible" as CFString)
            print("axprobe: depth AXScrollToVisible \"Row 0\" -> AXError \(err.rawValue)")
            settle()
        }
    }
}
