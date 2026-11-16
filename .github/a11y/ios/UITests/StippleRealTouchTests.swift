// Drives touchdemo (com.karpeleslab.stipple) with real touches: every tap,
// swipe, long press and drag below is an HID event the simulator delivers to
// the app as UITouches, through UIKit's hit testing and the app's view, not
// the synthetic touch path touchdemo's self-test calls in-process. What the
// touches did is read back from the app's accessibility tree (labels the app
// updates). Launched with TOUCHDEMO_SERVICES, touchdemo shows its services'
// controls: timers, the clipboard and opening a URL.
//
// Each part logs "stipple-touch: <part> passed" or "stipple-touch: <part>
// FAILED: ..." and the test goes on to the next, so CI reports each one.
// Before the test, CI puts "Pasted from simctl" on the simulator's
// pasteboard (xcrun simctl pbcopy), and says so in STIPPLE_PBCOPIED=1
// (xcodebuild passes TEST_RUNNER_-prefixed variables on to the runner);
// when simctl couldn't, the runner copies the text itself, as another app.
// After the test, CI reads the pasteboard back (xcrun simctl pbpaste), which
// must hold what the app's "Set clip" put there.
import UIKit
import XCTest

final class StippleRealTouchTests: XCTestCase {
    private var failures: [String] = []

    override func setUp() {
        continueAfterFailure = true
    }

    private func log(_ line: String) {
        print("stipple-touch: \(line)")
    }

    private func report(_ part: String, _ problem: String?) {
        if let problem = problem {
            log("\(part) FAILED: \(problem)")
            failures.append(part)
        } else {
            log("\(part) passed")
        }
    }

    /// The static text whose label starts with `prefix`.
    private func text(_ app: XCUIApplication, _ prefix: String) -> XCUIElement {
        app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", prefix)).firstMatch
    }

    /// Wait up to `timeout` seconds for `element`'s label to satisfy `ok`;
    /// returns whether it did and the last label read.
    private func waitLabel(
        _ element: XCUIElement, _ timeout: TimeInterval, _ ok: (String) -> Bool
    ) -> (Bool, String) {
        let end = Date().addingTimeInterval(timeout)
        var last = "(missing)"
        repeat {
            if element.exists {
                last = element.label
                if ok(last) {
                    return (true, last)
                }
            }
            Thread.sleep(forTimeInterval: 0.25)
        } while Date() < end
        return (false, last)
    }

    /// The number after `prefix` in `label`.
    private func number(_ label: String, _ prefix: String) -> Int? {
        guard label.hasPrefix(prefix) else { return nil }
        let rest = label.dropFirst(prefix.count).prefix { $0 == "-" || $0.isNumber }
        return Int(rest)
    }

    /// The rows wholly inside the list (between the "Rows tapped" label and
    /// the strip of chips), top to bottom.
    private func visibleRows(_ app: XCUIApplication) -> [XCUIElement] {
        let top = text(app, "Rows tapped: ").frame.maxY
        let chip = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Chip ")).firstMatch
        let bottom = chip.exists ? chip.frame.minY : app.frame.maxY
        let rows = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Row "))
        return rows.allElementsBoundByIndex
            .filter { $0.frame.minY >= top && $0.frame.maxY <= bottom && !$0.frame.isEmpty }
            .sorted { $0.frame.minY < $1.frame.minY }
    }

    private func listOffset(_ app: XCUIApplication) -> Int {
        number(text(app, "List offset: ").label, "List offset: ") ?? -1
    }

    private func rowsTapped(_ app: XCUIApplication) -> Int {
        number(text(app, "Rows tapped: ").label, "Rows tapped: ") ?? -1
    }

    /// Tap "Allow Paste" if iOS asks (in the app or from SpringBoard).
    private func allowPaste(_ app: XCUIApplication) -> Bool {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        for button in [app.buttons["Allow Paste"], springboard.buttons["Allow Paste"]]
        where button.exists {
            button.tap()
            log("answered the paste prompt (Allow Paste)")
            return true
        }
        return false
    }

    func testWithRealTouches() throws {
        let app = XCUIApplication(bundleIdentifier: "com.karpeleslab.stipple")
        app.launchEnvironment = ["TOUCHDEMO_SERVICES": "1"]
        app.launch()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 30), "the app didn't come up")
        guard app.buttons["Timer"].waitForExistence(timeout: 30) else {
            log("no \"Timer\" button: the services' controls aren't shown")
            print(app.debugDescription)
            XCTFail("touchdemo didn't show its services' controls (TOUCHDEMO_SERVICES)")
            return
        }

        // 1. Timers: the ticks an AppHandle::every timer counts change the
        // label on their own; a real tap on "Timer" arms an AppHandle::after
        // timer and starts a thread that wakes the loop (AppHandle::update).
        do {
            let ticks = text(app, "Ticks: ")
            let first = number(ticks.label, "Ticks: ") ?? -1
            let (counted, now) = waitLabel(ticks, 10) { (self.number($0, "Ticks: ") ?? -1) >= first + 2 }
            log("ticks went from \(first) to \"\(now)\"")
            app.buttons["Timer"].tap()
            let (fired, timer) = waitLabel(text(app, "Timer: "), 10) { $0 == "Timer: fired" }
            let (woke, worker) = waitLabel(text(app, "Worker: "), 10) { $0 == "Worker: done" }
            log("after the tap on \"Timer\": \"\(timer)\", \"\(worker)\"")
            report(
                "timers",
                !counted ? "the ticks label didn't count up (\"\(now)\")"
                    : !fired ? "the one-shot timer didn't fire (\"\(timer)\")"
                    : !woke ? "the worker thread's update didn't arrive (\"\(worker)\")" : nil)
        }

        // 2. The list, by finger: a swipe scrolls it (and taps no row), a
        // slow drag with XCUICoordinate scrolls it back, a tap at a row's
        // coordinate taps it, and a long press opens its context menu, whose
        // item a tap picks.
        do {
            var problem: String?
            let tapsBefore = rowsTapped(app)
            let start = listOffset(app)
            let rows = visibleRows(app)
            log("list offset \(start), \(rows.count) rows in view, rows tapped \(tapsBefore)")
            if let row = rows.dropFirst().first {
                row.swipeUp()
                Thread.sleep(forTimeInterval: 2)
                let (scrolled, label) = waitLabel(text(app, "List offset: "), 5) {
                    (self.number($0, "List offset: ") ?? 0) > start
                }
                let swiped = listOffset(app)
                log("after a swipe up: \"\(label)\", rows tapped \(rowsTapped(app))")
                if !scrolled {
                    problem = "the swipe didn't scroll the list (\(label))"
                } else if rowsTapped(app) != tapsBefore {
                    problem = "the swipe tapped a row"
                } else if let top = visibleRows(app).first {
                    let from = top.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
                    let to = from.withOffset(CGVector(dx: 0, dy: 160))
                    from.press(forDuration: 0.1, thenDragTo: to)
                    Thread.sleep(forTimeInterval: 1.5)
                    let back = listOffset(app)
                    log("after a drag down: list offset \(swiped) -> \(back)")
                    if back >= swiped {
                        problem = "the drag down didn't scroll the list back (\(swiped) -> \(back))"
                    }
                } else {
                    problem = "no row in view after the swipe"
                }
            } else {
                problem = "fewer than two rows in view"
            }
            report("list swipe and drag", problem)

            problem = nil
            let inView = visibleRows(app)
            if let row = inView.first {
                let before = rowsTapped(app)
                row.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.5)).tap()
                let (tapped, label) = waitLabel(text(app, "Rows tapped: "), 5) {
                    (self.number($0, "Rows tapped: ") ?? 0) == before + 1
                }
                log("tapped \"\(row.label)\" at its coordinate: \"\(label)\"")
                if !tapped {
                    problem = "the tap on \"\(row.label)\" didn't count (\"\(label)\")"
                }
            } else {
                problem = "no row in view"
            }
            report("row tap", problem)

            problem = nil
            if let row = visibleRows(app).dropFirst().first {
                let name = row.label
                let item = "Pin " + name.lowercased()  // "Row 4" -> "Pin row 4"
                let before = rowsTapped(app)
                row.press(forDuration: 1.2)
                let pin = app.descendants(matching: .any)
                    .matching(NSPredicate(format: "label == %@", item)).firstMatch
                if pin.waitForExistence(timeout: 5) {
                    log("the long press on \"\(name)\" opened its menu: \(pin.elementType.rawValue) \"\(pin.label)\"")
                    pin.tap()
                    let (pinned, label) = waitLabel(text(app, "Pinned: "), 5) { $0 == "Pinned: 1" }
                    log("after picking \"\(item)\": \"\(label)\"")
                    if !pinned {
                        problem = "picking \"\(item)\" didn't pin the row (\"\(label)\")"
                    } else if rowsTapped(app) != before {
                        problem = "the long press also tapped the row"
                    }
                } else {
                    print(app.debugDescription)
                    problem = "the long press on \"\(name)\" opened no \"\(item)\" item"
                }
            } else {
                problem = "no row in view"
            }
            report("long press", problem)
        }

        // 3. The clipboard into the app: the text field takes the finger,
        // a second tap on its caret brings up the system's edit menu
        // (UIEditMenuInteraction), whose Paste puts what simctl pbcopy left
        // on the pasteboard into the field. Failing that, the app's "Get
        // clip" button reads the pasteboard (platform::clipboard::text).
        do {
            let wanted = "Pasted from simctl"
            if ProcessInfo.processInfo.environment["STIPPLE_PBCOPIED"] != "1" {
                // Writing needs no permission; only reading another app's does.
                UIPasteboard.general.string = wanted
                log("simctl pbcopy didn't run: the runner copied \"\(wanted)\" itself")
            }
            let field = app.textFields.firstMatch
            var problem: String?
            var how = ""
            if field.waitForExistence(timeout: 5) {
                let before = (field.value as? String) ?? ""
                // The caret goes to the end of the text: tap there, then on it.
                let caret = field.coordinate(withNormalizedOffset: CGVector(dx: 0.0, dy: 0.5))
                    .withOffset(CGVector(dx: before.isEmpty ? 14 : field.frame.width - 14, dy: 0))
                caret.tap()
                Thread.sleep(forTimeInterval: 1.5)
                let paste = app.descendants(matching: .any)
                    .matching(NSPredicate(format: "label == %@", "Paste")).firstMatch
                if !paste.waitForExistence(timeout: 2) {
                    caret.tap()
                }
                if !paste.waitForExistence(timeout: 4) {
                    log("no edit menu after tapping the caret; trying a long press")
                    field.press(forDuration: 1.2)
                }
                if paste.waitForExistence(timeout: 4) {
                    log("edit menu's Paste: \(paste.elementType.rawValue) frame=\(paste.frame)")
                    paste.tap()
                    var now = before
                    for _ in 0..<40 {
                        _ = allowPaste(app)
                        now = (field.value as? String) ?? ""
                        if now.contains(wanted) {
                            break
                        }
                        Thread.sleep(forTimeInterval: 0.25)
                    }
                    log("text field value after Paste: \"\(now)\"")
                    if now.contains(wanted) {
                        how = "the edit menu's Paste"
                    }
                } else {
                    print(app.debugDescription)
                    log("no Paste item in an edit menu")
                }
            } else {
                log("no text field")
            }
            if how.isEmpty {
                // Away from the field (the keyboard may cover the buttons
                // otherwise), then the app's own button.
                text(app, "Rows tapped: ").tap()
                Thread.sleep(forTimeInterval: 1)
                app.buttons["Get clip"].tap()
                var label = ""
                for _ in 0..<40 {
                    _ = allowPaste(app)
                    label = text(app, "Clipboard: ").label
                    if label.contains(wanted) {
                        break
                    }
                    Thread.sleep(forTimeInterval: 0.25)
                }
                log("after \"Get clip\": \"\(label)\"")
                if label.contains(wanted) {
                    how = "the app's Get clip button"
                } else {
                    problem = "neither the edit menu nor \"Get clip\" got \"\(wanted)\" (\"\(label)\")"
                }
            }
            if problem == nil {
                log("pasted \"\(wanted)\" through \(how)")
            }
            report("paste from simctl", problem)
        }

        // 4. The clipboard out of the app: "Set clip" writes it; CI reads
        // the simulator's pasteboard afterwards with simctl pbpaste.
        do {
            text(app, "Rows tapped: ").tap()
            Thread.sleep(forTimeInterval: 1)
            let changes = UIPasteboard.general.changeCount
            app.buttons["Set clip"].tap()
            let (set, label) = waitLabel(text(app, "Clipboard: "), 5) { $0 == "Clipboard: set true" }
            // (Reading the text from here would ask for permission; the
            // change count and whether it holds a string don't.)
            log("after \"Set clip\": \"\(label)\", the pasteboard's change count \(changes) -> \(UIPasteboard.general.changeCount), hasStrings=\(UIPasteboard.general.hasStrings)")
            report("copy for simctl", set ? nil : "\"Set clip\" didn't write the clipboard (\"\(label)\")")
        }

        // 4b. Dragging out: a long press on "Rows tapped" (a drag source of a
        // text) lifts it through UIKit's drag interaction, a real lift; the
        // finger carries it onto the pinch box, which takes it through the
        // drop interaction as it would another app's drag: "Drops: N" counts
        // up.
        do {
            var problem: String?
            let drops = text(app, "Drops: ")
            let before = drops.exists ? drops.label : "(missing)"
            text(app, "Rows tapped: ").press(forDuration: 1.2, thenDragTo: text(app, "Pinch me"))
            let (dropped, label) = waitLabel(drops, 8) { $0 != before }
            log("after the lift: \"\(label)\" (was \"\(before)\")")
            if !dropped {
                problem = "the lifted text didn't drop on the pinch box (\"\(label)\")"
            }
            report("lift and drop", problem)
        }

        // 5. Opening a URL: a tap on "Open URL" (AppHandle::open_url) brings
        // Safari to the foreground; then the app comes back.
        do {
            var problem: String?
            app.buttons["Open URL"].tap()
            let safari = XCUIApplication(bundleIdentifier: "com.apple.mobilesafari")
            if safari.wait(for: .runningForeground, timeout: 30) {
                log("Safari came to the foreground")
                let mentions = safari.descendants(matching: .any).matching(
                    NSPredicate(format: "label CONTAINS[c] %@ OR value CONTAINS[c] %@", "example.com", "example.com")
                ).count
                log("Safari elements mentioning example.com: \(mentions)")
            } else {
                problem = "Safari didn't come to the foreground (state \(safari.state.rawValue))"
            }
            app.activate()
            if app.wait(for: .runningForeground, timeout: 20) {
                let (opened, label) = waitLabel(text(app, "Open: "), 5) { $0 == "Open: Ok(())" }
                log("back in the app: \"\(label)\"")
                if !opened && problem == nil {
                    problem = "the app's open_url said \"\(label)\""
                }
            } else if problem == nil {
                problem = "the app didn't come back to the foreground"
            }
            report("open URL", problem)
        }

        log(failures.isEmpty ? "passed" : "FAILED: \(failures.joined(separator: ", "))")
        XCTAssertTrue(failures.isEmpty, "parts failed: \(failures)")
    }
}
