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

    func testDefaultIsCommutator() {
        let app = launch()
        XCTAssertTrue(app.tabBars.firstMatch.waitForExistence(timeout: 15))
        XCTAssertGreaterThan(backgroundHits(app, commutatorDark), 10, "the background is Commutator Dark's")
    }
}
