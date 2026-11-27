import XCTest

/// Signed out, "Try the demo" opens the offline demo workspace (App Review's
/// way in, the server only letting in the accounts it lists).
final class TryDemoTests: XCTestCase {
    func testTryTheDemoFromSignIn() {
        continueAfterFailure = false
        let app = XCUIApplication()
        app.launchArguments = ["-signedout"]
        app.launch()
        let demo = app.buttons["try-demo"]
        XCTAssertTrue(demo.waitForExistence(timeout: 10))
        demo.tap()
        // Chats or Projects, whichever this phone last showed: both are "Sessions".
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 15))
        XCTAssertFalse(demo.exists)
    }
}
