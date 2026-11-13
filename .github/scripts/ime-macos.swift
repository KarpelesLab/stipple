// Test tooling for typing through a real macOS input method (CI only).
//
//   swiftc -O -o ime-macos .github/scripts/ime-macos.swift
//   ./ime-macos list                 input sources, enabled and selected
//   ./ime-macos select <id>...       enable each, select the last
//   ./ime-macos current              the selected keyboard input source
//   ./ime-macos trusted              whether this process may post events
//   ./ime-macos keys <key>...        post key presses (CGEvent, HID tap)
//
// Keys are letters, digits, or tab, space, return, escape, delete, kana,
// eisu; "+200" waits 200 ms.
import ApplicationServices
import Carbon
import CoreGraphics
import Foundation

func sources(_ id: String?, all: Bool) -> [TISInputSource] {
  var filter: [String: Any] = [:]
  if let id { filter[kTISPropertyInputSourceID as String] = id }
  guard let list = TISCreateInputSourceList(filter as CFDictionary, all)?.takeRetainedValue()
  else { return [] }
  return (list as NSArray).map { $0 as! TISInputSource }
}

func prop<T>(_ s: TISInputSource, _ key: CFString) -> T? {
  guard let p = TISGetInputSourceProperty(s, key) else { return nil }
  return Unmanaged<AnyObject>.fromOpaque(p).takeUnretainedValue() as? T
}

func describe(_ s: TISInputSource) -> String {
  let id: String = prop(s, kTISPropertyInputSourceID) ?? "?"
  let enabled: Bool = prop(s, kTISPropertyInputSourceIsEnabled) ?? false
  let selected: Bool = prop(s, kTISPropertyInputSourceIsSelected) ?? false
  let selectable: Bool = prop(s, kTISPropertyInputSourceIsSelectCapable) ?? false
  return "\(id) enabled=\(enabled) selected=\(selected) selectable=\(selectable)"
}

func current() -> String {
  describe(TISCopyCurrentKeyboardInputSource().takeRetainedValue())
}

let keycodes: [String: CGKeyCode] = [
  "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7, "c": 8, "v": 9,
  "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16, "t": 17, "1": 18, "2": 19,
  "3": 20, "4": 21, "6": 22, "5": 23, "9": 25, "7": 26, "8": 28, "0": 29, "o": 31,
  "u": 32, "i": 34, "p": 35, "l": 37, "j": 38, "k": 40, "n": 45, "m": 46,
  "return": 36, "tab": 48, "space": 49, "delete": 51, "escape": 53,
  "eisu": 102, "kana": 104,
]

func post(_ name: String) -> Bool {
  if name.hasPrefix("+"), let ms = UInt32(name.dropFirst()) {
    usleep(ms * 1000)
    return true
  }
  guard let code = keycodes[name] else {
    print("ime-macos: unknown key \(name)")
    return false
  }
  let src = CGEventSource(stateID: .hidSystemState)
  for down in [true, false] {
    guard let ev = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: down) else {
      print("ime-macos: no event for \(name)")
      return false
    }
    ev.post(tap: .cghidEventTap)
    usleep(30_000)
  }
  usleep(120_000)
  return true
}

let args = Array(CommandLine.arguments.dropFirst())
switch args.first {
case "list":
  for s in sources(nil, all: true) { print(describe(s)) }
case "current":
  print(current())
case "trusted":
  print("ime-macos: AXIsProcessTrusted=\(AXIsProcessTrusted())")
case "select":
  var status: OSStatus = 0
  for id in args.dropFirst() {
    let found = sources(id, all: true)
    if found.isEmpty { print("ime-macos: no input source \(id)") }
    for s in found {
      let e = TISEnableInputSource(s)
      print("ime-macos: enable \(id): \(e)")
      if e != noErr { status = e }
    }
  }
  if let last = args.dropFirst().last, let s = sources(last, all: false).first {
    let r = TISSelectInputSource(s)
    print("ime-macos: select \(last): \(r)")
    if r != noErr { status = r }
  }
  // The selection lands through the run loop.
  RunLoop.current.run(until: Date().addingTimeInterval(1))
  print("ime-macos: current \(current())")
  exit(status == noErr ? 0 : 1)
case "keys":
  var ok = true
  for k in args.dropFirst() { ok = post(k) && ok }
  exit(ok ? 0 : 1)
default:
  print("usage: ime-macos list|current|trusted|select <id>...|keys <key>...")
  exit(2)
}
