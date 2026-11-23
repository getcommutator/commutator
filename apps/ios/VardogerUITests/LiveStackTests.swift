import XCTest

/// Against a real stack: a local vardoger-server and an engine signed in to
/// it, running the mock harness (crates/client/examples/remote_stack.rs,
/// which prints the phone's account and machine). Skipped unless
/// `TEST_RUNNER_VARDOGER_LIVE_ACCOUNT="<server url> <token> <device id>"` and
/// `TEST_RUNNER_VARDOGER_LIVE_HOST=<machine id>` are set:
///
///   cargo run -p vardoger-client --example remote_stack -- /tmp/stack &
///   TEST_RUNNER_VARDOGER_LIVE_ACCOUNT=… TEST_RUNNER_VARDOGER_LIVE_HOST=… xcodebuild test \
///     -only-testing:VardogerUITests/LiveStackTests …
final class LiveStackTests: XCTestCase {
    func testSendRoundTripsThroughRealEngine() throws {
        let env = ProcessInfo.processInfo.environment
        let account = try XCTUnwrap(env["VARDOGER_LIVE_ACCOUNT"], "set TEST_RUNNER_VARDOGER_LIVE_ACCOUNT").split(separator: " ").map(String.init)
        let host = try XCTUnwrap(env["VARDOGER_LIVE_HOST"], "set TEST_RUNNER_VARDOGER_LIVE_HOST")
        let app = XCUIApplication()
        // The draft's harness wins over the engine's VARDOGER_HARNESS default:
        // pin the mock so the test never starts a real agent.
        app.launchArguments = ["-signedout", "-account"] + account + ["-host", host, "-harness", "mock"]
        app.launch()

        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 15))
        accessory.tap()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        input.typeText("hello from the phone")
        app.buttons["composer-send"].tap()

        // The engine adopts the command (echo keeps its id) and the mock
        // harness replies through the chat2 room.
        let echo = app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'You: hello from the phone'")).firstMatch
        XCTAssertTrue(echo.waitForExistence(timeout: 10))
        let reply = app.staticTexts.matching(identifier: "row-markdown").firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 60), "mock harness reply streamed back")
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = "live-roundtrip"
        shot.lifetime = .keepAlways
        add(shot)
        XCTAssertTrue(app.buttons["Send message"].waitForExistence(timeout: 60), "turn settles")
    }
}
