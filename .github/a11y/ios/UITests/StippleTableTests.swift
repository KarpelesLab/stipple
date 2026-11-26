// Reads touchdemo's table and captioned number input (TOUCHDEMO_RANGES) from
// the XCUITest runner, a separate process, through the AX channel VoiceOver
// uses: the table's element is a data table
// (UIAccessibilityContainerDataTable), which XCUITest reports as an element
// ("Items") holding its cells, and the "Level" number input is labelled by its
// caption. Lines starting "stipple-ax-table:" are what CI checks.
import XCTest

final class StippleTableTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = true
    }

    private func log(_ line: String) {
        print("stipple-ax-table: \(line)")
    }

    func testTableAndCaption() throws {
        let app = XCUIApplication(bundleIdentifier: "com.karpeleslab.stipple")
        app.launchEnvironment = ["TOUCHDEMO_RANGES": "1"]
        app.launch()
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 30), "the app didn't come up")
        XCTAssertTrue(app.buttons["Row 0"].waitForExistence(timeout: 30), "no \"Row 0\" button")
        print(app.debugDescription)
        var failed: [String] = []

        // 1. The table ("Items"): an element holding its cells, which the
        // data table vends (XCUITest types a data table container as an
        // "other" element, not a table: that's UITableView's).
        let table = app.descendants(matching: .any)["Items"]
        if table.waitForExistence(timeout: 10) {
            log("table type=\(table.elementType.rawValue) (tables=\(app.tables.count))")
            let cells = table.cells.count
            let texts = table.staticTexts.count
            let any = table.descendants(matching: .any).count
            log("table frame=\(table.frame) cells=\(cells) staticTexts=\(texts) descendants=\(any)")
            let item = table.descendants(matching: .any)
                .matching(NSPredicate(format: "label BEGINSWITH %@", "Item ")).firstMatch
            if item.exists {
                log("table holds \"\(item.label)\"")
            } else {
                failed.append("no \"Item N\" inside the table")
            }
        } else {
            log("tables=\(app.tables.count)")
            failed.append("no table element")
        }

        // 2. The captioned number input reads its caption.
        // (A spin button's element is a text field: its text is its value.)
        let level = app.textFields["Level"]
        if level.waitForExistence(timeout: 5) {
            log("captioned text field \"\(level.label)\" value=\(String(describing: level.value))")
        } else {
            log("text fields: \(app.textFields.allElementsBoundByIndex.map { $0.label })")
            failed.append("no text field labelled \"Level\"")
        }

        if failed.isEmpty {
            log("passed")
        } else {
            log("FAILED: \(failed.joined(separator: "; "))")
            XCTFail(failed.joined(separator: "; "))
        }
    }
}
