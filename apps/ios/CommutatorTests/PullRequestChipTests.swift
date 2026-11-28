import XCTest
@testable import Commutator

/// Label chips on a pull request row: whole chips that fit, then `+N`.
final class PullRequestChipTests: XCTestCase {
    private let more: (Int) -> CGFloat = { _ in 20 }

    func testAllFit() {
        XCTAssertEqual(PullRequestCell.fitting([50, 50], room: 200, more: more), 2)
    }

    func testLeavesRoomForTheCount() {
        // 50 + 6 + 50 = 106 fits alone, but not with + 6 + 20 for the third.
        XCTAssertEqual(PullRequestCell.fitting([50, 50, 50], room: 120, more: more), 1)
    }

    func testNoneFit() {
        XCTAssertEqual(PullRequestCell.fitting([170], room: 100, more: more), 0)
        XCTAssertEqual(PullRequestCell.fitting([], room: 100, more: more), 0)
    }
}
