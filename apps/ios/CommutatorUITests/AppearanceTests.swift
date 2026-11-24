import XCTest

/// Themes: the app paints with the selected theme, from launch and after a change in Settings.
final class AppearanceTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    /// Dracula's background (crates/theme builtins).
    private let dracula: UInt32 = 0x282A36
    /// Commutator Dark's background.
    private let commutatorDark: UInt32 = 0x060606

    /// How many points down the left edge of the screen show `rgb` (± a little for colour
    /// management): the list's background shows there, outside the rows' insets.
    private func backgroundHits(_ app: XCUIApplication, _ rgb: UInt32) -> Int {
        let image = app.screenshot().image
        guard let cg = image.cgImage else { return 0 }
        let (w, h) = (cg.width, cg.height)
        let space = CGColorSpace(name: CGColorSpace.sRGB)!
        var data = [UInt8](repeating: 0, count: w * h * 4)
        let ctx = CGContext(data: &data, width: w, height: h, bitsPerComponent: 8, bytesPerRow: w * 4,
                            space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!
        ctx.draw(cg, in: CGRect(x: 0, y: 0, width: w, height: h))
        let want = [Int(rgb >> 16 & 0xFF), Int(rgb >> 8 & 0xFF), Int(rgb & 0xFF)]
        var hits = 0
        for i in 0..<20 {
            let y = h * (30 + i * 2) / 100
            let p = (y * w + 2) * 4
            let got = [Int(data[p]), Int(data[p + 1]), Int(data[p + 2])]
            if zip(got, want).allSatisfy({ abs($0 - $1) <= 6 }) { hits += 1 }
        }
        return hits
    }

    private func launch(_ extra: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        // `-appearance 2`: dark (UserDefaults argument domain, like `-theme.dark`).
        app.launchArguments = ["-demo", "-fast", "-appearance", "2"] + extra
        app.launch()
        return app
    }

    func testThemeAtLaunch() {
        let app = launch(["-theme.dark", "dracula"])
        XCTAssertTrue(app.tabBars.firstMatch.waitForExistence(timeout: 15))
        XCTAssertGreaterThan(backgroundHits(app, dracula), 10, "the background is Dracula's")
    }

    func testPickDarkTheme() {
        let app = launch()
        let settings = app.tabBars.buttons["Settings"]
        XCTAssertTrue(settings.waitForExistence(timeout: 15))
        settings.tap()
        let appearance = app.cells["settings-appearance"]
        XCTAssertTrue(appearance.waitForExistence(timeout: 5))
        appearance.tap()
        app.cells["settings-theme:dark"].tap()
        let dracula = app.cells["settings-variant:dracula"]
        XCTAssertTrue(dracula.waitForExistence(timeout: 5))
        dracula.tap()
        // Repainted in place, no relaunch.
        XCTAssertGreaterThan(backgroundHits(app, self.dracula), 10, "the background turned Dracula's")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(appearance.staticTexts.containing(NSPredicate(format: "label CONTAINS 'Dracula'")).firstMatch.waitForExistence(timeout: 5))

        // Back to Commutator, so other tests start from the default.
        appearance.tap()
        app.cells["settings-theme:dark"].tap()
        app.cells["settings-variant:commutator-dark"].tap()
        XCTAssertGreaterThan(backgroundHits(app, commutatorDark), 10, "back to Commutator Dark")
    }

    func testDefaultIsCommutator() {
        let app = launch(["-theme.dark", "commutator-dark"])
        XCTAssertTrue(app.tabBars.firstMatch.waitForExistence(timeout: 15))
        XCTAssertGreaterThan(backgroundHits(app, commutatorDark), 10, "the background is Commutator Dark's")
    }
}
