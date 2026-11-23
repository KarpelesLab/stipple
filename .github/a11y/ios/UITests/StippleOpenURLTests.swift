// Opens touchdemo's custom scheme (stipple-demo://, which the bundle CI
// installs claims in CFBundleURLTypes) as a link would, through the system
// (XCUIDevice.system.open, LaunchServices), first with the app not running
// (the URL launches it), then again while it runs (`simctl openurl` never
// started the app on CI's simulator). If iOS asks before the custom scheme
// opens the app ("Open in “Stipple”?"), SpringBoard's "Open" is tapped.
// Whether the app heard each URL is read from its log by CI (touchdemo logs
// `touchdemo: opened url "..."`); this test only checks the app came to the
// foreground. Each step logs "stipple-url: ...".
import XCTest

final class StippleOpenURLTests: XCTestCase {
    private func log(_ line: String) {
        print("stipple-url: \(line)")
    }

    /// Open `link` through the system and answer the prompt, if iOS asks.
    private func open(_ link: String) {
        guard let url = URL(string: link) else {
            XCTFail("bad URL \(link)")
            return
        }
        guard #available(iOS 16.4, *) else {
            XCTFail("XCUIDevice.system.open needs iOS 16.4")
            return
        }
        log("opening \(link)")
        XCUIDevice.shared.system.open(url)
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let button = springboard.buttons["Open"]
        if button.waitForExistence(timeout: 8) {
            log("iOS asked to open it in the app: answered Open")
            button.tap()
        } else {
            log("iOS didn't ask")
        }
    }

    func testOpenURLs() {
        continueAfterFailure = true
        let app = XCUIApplication(bundleIdentifier: "com.karpeleslab.stipple")
        app.terminate()
        // Cold: the URL launches the app.
        open("stipple-demo://callback?code=1")
        let cold = app.wait(for: .runningForeground, timeout: 30)
        log("after the first URL: app state \(app.state.rawValue)")
        XCTAssertTrue(cold, "the first URL didn't bring the app up")
        Thread.sleep(forTimeInterval: 3)
        // Warm: the running app hears the next one.
        open("stipple-demo://callback?code=2")
        let warm = app.wait(for: .runningForeground, timeout: 30)
        log("after the second URL: app state \(app.state.rawValue)")
        XCTAssertTrue(warm, "the app wasn't in the foreground after the second URL")
        Thread.sleep(forTimeInterval: 3)
    }
}
