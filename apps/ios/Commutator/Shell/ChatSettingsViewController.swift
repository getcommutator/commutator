import UIKit

/// Settings › Chat: how a transcript reads. Math for now, as the desktop's Chat section.
final class ChatSettingsViewController: SettingsListController {
    init() {
        super.init(nibName: nil, bundle: nil)
        title = "Chat"
    }

    required init?(coder: NSCoder) { fatalError() }

    /// The Chat row's summary in Settings.
    static var summary: String {
        MathSettings.render ? "Math in \(MathSettings.label(MathSettings.face))" : "Math as source"
    }

    override func toggled(_ id: String, _ on: Bool) {
        if id == "math:render" {
            MathSettings.render = on
            reload()
        }
    }

    override func sections() -> [(title: String, rows: [Row])] {
        var math = [Row(id: "math:render", title: "Render Math", subtitle: "Off shows LaTeX as its source", symbol: "function", accessory: .toggle(MathSettings.render))]
        if MathSettings.render {
            let face = MathSettings.face
            math.append(Row(id: "math:font", title: "Math Font", subtitle: "\(MathSettings.label(face)) — \(MathSettings.detail(face))", symbol: "textformat"))
            math.append(Row(id: "math:size", title: "Math Size", subtitle: "\(MathSettings.size)% of the text around it", symbol: "textformat.size"))
        }
        return [("Math", math)]
    }

    override func didSelect(_ row: Row, at path: IndexPath) {
        switch row.id {
        case "math:font":
            pick("Math Font", MathSettings.allFaces, label: MathSettings.label, current: MathSettings.face, at: path) {
                MathSettings.face = $0
            }
        case "math:size":
            pick("Math Size", MathSettings.allSizes, label: { "\($0)%" }, current: MathSettings.size, at: path) {
                MathSettings.size = $0
            }
        default:
            break
        }
    }

    /// An action sheet of `options`, the current one checked.
    private func pick<T: Equatable>(_ title: String, _ options: [T], label: (T) -> String, current: T, at path: IndexPath, set: @escaping (T) -> Void) {
        let sheet = UIAlertController(title: title, message: nil, preferredStyle: .actionSheet)
        for option in options {
            let action = UIAlertAction(title: label(option), style: .default) { [weak self] _ in
                set(option)
                self?.reload()
            }
            action.setValue(option == current, forKey: "checked")
            sheet.addAction(action)
        }
        sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        sheet.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
        present(sheet, animated: true)
    }
}
