// Drives touchdemo (installed as com.karpeleslab.stipple) from the XCUITest
// runner, a separate process: every query and action below goes through the
// app's accessibility elements over the AX channel VoiceOver uses, not through
// anything inside the app. Lines starting "stipple-ax:" are what CI checks.
import XCTest

final class StippleAccessibilityTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func log(_ line: String) {
        print("stipple-ax: \(line)")
    }

    private func describe(_ e: XCUIElement) -> String {
        let value = (e.value as? String).map { " value=\"\($0)\"" } ?? ""
        return "label=\"\(e.label)\"\(value) frame=\(e.frame)"
    }

    func testThroughAccessibility() throws {
        let app = XCUIApplication(bundleIdentifier: "com.karpeleslab.stipple")
        app.launch()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 30), "the app didn't come up")

        // 1. The tree, as an assistive client reads it.
        let row0 = app.buttons["Row 0"]
        XCTAssertTrue(row0.waitForExistence(timeout: 30), "no \"Row 0\" button in the tree")
        let any = app.descendants(matching: .any).count
        log("elements any=\(any) buttons=\(app.buttons.count) staticTexts=\(app.staticTexts.count) textFields=\(app.textFields.count)")
        print(app.debugDescription)
        let screen = app.frame
        log("app frame=\(screen)")
        log("button \(describe(row0))")
        XCTAssertFalse(row0.frame.isEmpty, "\"Row 0\" has an empty frame")
        XCTAssertTrue(screen.contains(row0.frame), "\"Row 0\"'s frame is off the screen")
        XCTAssertTrue(row0.isHittable, "\"Row 0\" isn't hittable at its frame")
        let label = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH %@", "Rows tapped: ")).firstMatch
        XCTAssertTrue(label.exists, "no \"Rows tapped: N\" static text")
        log("static text \(describe(label))")
        let before = Int(label.label.dropFirst("Rows tapped: ".count)) ?? -1
        XCTAssertGreaterThanOrEqual(before, 0, "can't read the count from \"\(label.label)\"")

        // 2. A tap on the button at its accessibility frame: the app counts
        // it, and the label's element follows.
        row0.tap()
        let after = "Rows tapped: \(before + 1)"
        XCTAssertTrue(
            app.staticTexts[after].waitForExistence(timeout: 10),
            "the tap on \"Row 0\" didn't reach the app (or the label's element didn't follow)")
        log("tapped \"Row 0\": the label now reads \"\(after)\"")

        // 3. The text field is a text field, takes the keyboard, and reports
        // what is typed as its value.
        let field = app.textFields.firstMatch
        XCTAssertTrue(field.waitForExistence(timeout: 10), "no element of type textField")
        log("text field \(describe(field))")
        let start = (field.value as? String) ?? ""
        field.tap()
        let keyboard = app.keyboards.firstMatch
        if keyboard.waitForExistence(timeout: 10) {
            log("software keyboard shown with \(keyboard.keys.count) keys")
            // The swipe-typing introduction can cover the keyboard the first
            // time it shows.
            let proceed = app.buttons["Continue"]
            if proceed.exists {
                proceed.tap()
            }
            for ch in ["H", "i"] {
                let key = [ch, ch.lowercased(), ch.uppercased()]
                    .map { keyboard.keys[$0] }
                    .first { $0.exists }
                XCTAssertNotNil(key, "no \(ch) key on the keyboard")
                key?.tap()
            }
            log("typed on the software keyboard's keys")
        } else {
            log("no software keyboard: typing with typeText")
            field.typeText("Hi")
        }
        // The field may hold text already (saved state); what was typed is
        // one more "h" and one more "i" (either case: the keyboard may
        // not be shifted).
        func count(_ s: String, _ c: Character) -> Int { s.lowercased().filter { $0 == c }.count }
        func typed(_ now: String) -> Bool {
            now.count == start.count + 2 && count(now, "h") == count(start, "h") + 1
                && count(now, "i") == count(start, "i") + 1
        }
        var now = start
        for _ in 0..<20 {
            now = (field.value as? String) ?? ""
            if typed(now) {
                break
            }
            Thread.sleep(forTimeInterval: 0.5)
        }
        XCTAssertTrue(typed(now), "the field's value is \"\(now)\" after typing \"Hi\" into \"\(start)\"")
        log("text field value \"\(now)\"")
        log("passed")
    }
}
