import UIKit

/// Sleep → Custom…: the system date picker (calendar and time), opening on
/// tomorrow at 9, never earlier than a minute from now. Cancel, or Sleep.
final class SleepPickerViewController: UIViewController {
    private let chatTitle: String?
    private let onSleep: (Date) -> Void
    private let picker = UIDatePicker()
    private let summary = UILabel()

    init(title chatTitle: String?, onSleep: @escaping (Date) -> Void) {
        self.chatTitle = chatTitle
        self.onSleep = onSleep
        super.init(nibName: nil, bundle: nil)
        title = "Sleep Until"
        modalPresentationStyle = .formSheet
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .cancel, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        let sleep = UIBarButtonItem(title: "Sleep", primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            let until = max(self.picker.date, Date().addingTimeInterval(60))
            self.dismiss(animated: true)
            self.onSleep(until)
        })
        sleep.style = .done
        sleep.accessibilityIdentifier = "sleep-confirm"
        navigationItem.rightBarButtonItem = sleep

        let now = Date()
        picker.datePickerMode = .dateAndTime
        picker.preferredDatePickerStyle = .inline
        picker.minimumDate = now.addingTimeInterval(60)
        picker.date = ActivityList.customStart(now: now)
        picker.minuteInterval = 5
        picker.accessibilityIdentifier = "sleep-picker"
        picker.addAction(UIAction { [weak self] _ in self?.updateSummary() }, for: .valueChanged)

        summary.font = .preferredFont(forTextStyle: .footnote)
        summary.textColor = Palette.secondary
        summary.numberOfLines = 0
        summary.adjustsFontForContentSizeCategory = true

        let card = UIView()
        card.backgroundColor = Palette.elevated
        card.layer.cornerRadius = 26
        card.layer.cornerCurve = .continuous
        picker.translatesAutoresizingMaskIntoConstraints = false
        card.addSubview(picker)
        let stack = UIStackView(arrangedSubviews: [card, summary])
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        let scroll = UIScrollView()
        scroll.alwaysBounceVertical = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        view.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: view.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 16),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -24),
            stack.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor, constant: -16),
            picker.topAnchor.constraint(equalTo: card.topAnchor, constant: 8),
            picker.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -8),
            picker.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 8),
            picker.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -8),
        ])
        updateSummary()
        if let sheet = sheetPresentationController ?? navigationController?.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
    }

    /// What it'll do, in a sentence under the picker.
    private func updateSummary() {
        let when = picker.date.formatted(.dateTime.weekday(.wide).month(.abbreviated).day().hour().minute())
        let name = chatTitle.map { "“\($0)”" } ?? "This chat"
        summary.text = "\(name) moves to Asleep until \(when), on every device. It wakes early if it asks you something, needs an approval or fails."
    }
}
