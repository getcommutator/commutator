import UIKit

/// One process the session's agent left running: the tail of its output,
/// following the end while it runs (read once a second, on screen only), and
/// Stop. It stays listed until the agent reports it gone; then it reads
/// "Ended" and the last output stays.
final class BackgroundOutputViewController: UIViewController {
    private let app: AppModel
    private let chatId: String
    private let task: BackgroundTaskRow
    private let text = UITextView()
    private var ticker: Timer?
    private var reading = false
    private var stopping = false
    private var ended = false
    private var shown: BackgroundOutput?

    init(app: AppModel, chatId: String, task: BackgroundTaskRow) {
        self.app = app
        self.chatId = chatId
        self.task = task
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.title = task.command.split(separator: "\n", maxSplits: 1).first.map(String.init) ?? task.command
        text.isEditable = false
        text.backgroundColor = .clear
        text.font = Fonts.ui(.mono, 12.5)
        text.textColor = Palette.text
        text.textContainerInset = UIEdgeInsets(top: 12, left: 14, bottom: 24, right: 14)
        text.alwaysBounceVertical = true
        text.text = "No output yet"
        text.accessibilityIdentifier = "background-output"
        text.frame = view.bounds
        text.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(text)
        setContentScrollView(text, for: .top)
        syncChrome()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        read()
        ticker?.invalidate()
        ticker = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.read() }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        ticker?.invalidate()
    }

    private func syncChrome() {
        let elapsed = SubagentFormat.elapsed(since: Date(timeIntervalSince1970: Double(task.startedAtMs) / 1000))
        navigationItem.subtitle = ended ? "Ended" : stopping ? "Stopping…" : task.description.map { "\($0) · \(elapsed)" } ?? "Running · \(elapsed)"
        guard !ended else {
            navigationItem.rightBarButtonItem = nil
            return
        }
        let stop = UIBarButtonItem(title: "Stop", image: UIImage(systemName: "stop.fill"), primaryAction: UIAction { [weak self] _ in
            self?.stop()
        })
        stop.tintColor = Palette.danger
        stop.isEnabled = !stopping
        stop.accessibilityIdentifier = "background-stop"
        navigationItem.rightBarButtonItem = stop
    }

    private func read() {
        guard !reading, !ended else { return }
        // Gone from the session's list: the agent reported it ended.
        if !app.sessionBackground(chatId).contains(where: { $0.id == task.id }) {
            ended = true
            ticker?.invalidate()
            syncChrome()
            return
        }
        reading = true
        Task { @MainActor in
            defer { reading = false }
            if let output = try? await app.backgroundOutput(chatId, id: task.id) {
                show(output)
            }
            syncChrome()
        }
    }

    private func show(_ output: BackgroundOutput) {
        guard output != shown else { return }
        // Follow the end unless the reader scrolled up.
        let atEnd = text.contentOffset.y + text.bounds.height >= text.contentSize.height - 40
        shown = output
        let body = output.text.isEmpty ? "No output yet" : output.text
        text.text = output.truncated ? "Earlier output not shown\n\n\(body)" : body
        if atEnd {
            text.layoutIfNeeded()
            let bottom = max(0, text.contentSize.height - text.bounds.height + text.adjustedContentInset.bottom)
            text.setContentOffset(CGPoint(x: 0, y: bottom), animated: false)
        }
    }

    private func stop() {
        guard !stopping else { return }
        stopping = true
        syncChrome()
        UIImpactFeedbackGenerator(style: .medium).impactOccurred()
        Task { @MainActor in
            do {
                _ = try await app.stopBackgroundTask(chatId, id: task.id)
            } catch {
                stopping = false
                syncChrome()
                Toast.show("Couldn't stop that process", in: view.window)
            }
        }
    }
}
