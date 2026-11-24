import SafariServices
import UIKit

/// How transcripts show LaTeX math, local to this phone: rendered (in one of
/// the core's bundled math faces) or as its TeX source.
enum MathSettings {
    static let didChange = Notification.Name("CommutatorMathSettingsChanged")

    /// Off shows `$…$` as an inline code chip and `$$…$$` as a code block.
    static var render: Bool {
        get { UserDefaults.standard.object(forKey: "mathRender") as? Bool ?? true }
        set {
            UserDefaults.standard.set(newValue, forKey: "mathRender")
            NotificationCenter.default.post(name: didChange, object: nil)
        }
    }

    /// Stored by `key`; anything unknown reads as Fira Math.
    static var face: MathFace {
        get {
            let stored = UserDefaults.standard.string(forKey: "mathFont")
            return allFaces.first { key($0) == stored } ?? .fira
        }
        set {
            UserDefaults.standard.set(key(newValue), forKey: "mathFont")
            NotificationCenter.default.post(name: didChange, object: nil)
        }
    }

    static let allFaces: [MathFace] = [.fira, .stixTwo, .computerModern, .libertinus, .pagella, .euler]

    private static func key(_ f: MathFace) -> String {
        switch f {
        case .fira: "fira"
        case .stixTwo: "stixTwo"
        case .computerModern: "computerModern"
        case .libertinus: "libertinus"
        case .pagella: "pagella"
        case .euler: "euler"
        }
    }

    /// Formula size as a percent of the text around it (the desktop's
    /// options; Rust clamps to 75–200).
    static var size: Int {
        get { UserDefaults.standard.object(forKey: "mathSize") as? Int ?? 100 }
        set {
            UserDefaults.standard.set(newValue, forKey: "mathSize")
            NotificationCenter.default.post(name: didChange, object: nil)
        }
    }

    static let allSizes = [90, 100, 110, 120, 135, 150]

    static func label(_ f: MathFace) -> String {
        switch f {
        case .fira: "Fira Math"
        case .stixTwo: "STIX Two"
        case .computerModern: "Computer Modern"
        case .libertinus: "Libertinus"
        case .pagella: "Palatino"
        case .euler: "Euler"
        }
    }

    static func detail(_ f: MathFace) -> String {
        switch f {
        case .fira: "A sans that sits beside the interface font."
        case .stixTwo: "A serif with the Times-like TeX look."
        case .computerModern: "The classic LaTeX look."
        case .libertinus: "A warm book serif."
        case .pagella: "TeX Gyre Pagella, a Palatino serif."
        case .euler: "Zapf's upright, hand-drawn math."
        }
    }

    static func apply(to engine: TranscriptView) {
        engine.setMath(enabled: render, face: face, sizePercent: UInt16(clamping: size))
    }
}

/// Virtualized transcript host. Rust owns geometry: each `LayoutFrame` gives
/// exact row offsets, so this view only (1) asks which rows intersect the
/// viewport, (2) positions reusable `RowView`s there, and (3) keeps the user's
/// place across frames (anchor or follow-the-tail).
final class TranscriptListView: UIScrollView, RowViewDelegate, UIScrollViewDelegate {
    let engine: TranscriptView
    let fonts = StyleFonts()
    private(set) var current: LayoutFrame?
    private var visible: [UInt64: RowView] = [:]
    private var pool: [RowView] = []
    private var cache: [ModelKey: RowModel] = [:]
    /// Agent visuals by id: one live view each, moved between reused rows so
    /// its painted state survives row reconfigures.
    private var visuals: [String: VisualView] = [:]
    private var memoryObserver: NSObjectProtocol?
    private var mathObserver: NSObjectProtocol?

    deinit {
        if let memoryObserver { NotificationCenter.default.removeObserver(memoryObserver) }
        if let mathObserver { NotificationCenter.default.removeObserver(mathObserver) }
    }
    private var cacheOrder: [ModelKey] = []
    private var knownKeys = Set<UInt64>()
    private var inflight = Set<ModelKey>()
    private let prefetchQueue = DispatchQueue(label: "dev.freethinkel.commutator.transcript.prefetch", qos: .userInitiated)

    /// Rows are realized this far beyond the viewport (and prefetched further).
    var overscan: CGFloat = 700

    /// Attachment upload progress: rings on pending thumbnails.
    var uploadProgress: Double? {
        didSet {
            guard uploadProgress != oldValue else { return }
            for v in visible.values { v.uploadProgress = uploadProgress }
        }
    }

    /// Following the tail: new content keeps the bottom in view.
    private(set) var following = true
    var onFollowChange: ((Bool) -> Void)?
    /// A row's button was tapped (`sign-in` on an error card).
    var onAction: ((String) -> Void)?
    /// A subagent card was tapped (doc id, title, still running).
    var onSubagent: ((String, String, Bool) -> Void)?
    /// An agent message's sender caption was tapped (the sending chat's id).
    var onOpenChat: ((String) -> Void)?
    var onDistanceFromBottom: ((CGFloat) -> Void)?
    var imageLoader: ((String, UIImageView) -> Void)?
    /// An attachment's picture, for an image chip opened full size.
    var imageFetcher: ((String) async -> UIImage?)?

    // MARK: Runway (desktop transcript.rs `OwnTurnAnchor`)
    //
    // On an immediate send the prompt glides to the top of the viewport and
    // the content is held at least one viewport tall below it, so the reply
    // streams into reserved space without the view moving on every token.
    // The reply consumes the reservation; once it fills it, the runway retires
    // and the normal follow-the-tail spring takes over. A drag releases the
    // hold (the reservation stays as plain scroll room); coming back to the
    // bottom — or Jump to latest — glides back to the held position.
    private struct OwnTurn {
        /// The prompt row, once its (optimistic) echo is laid out.
        var key: UInt64?
        /// User rows that existed at send time (the new one is the prompt).
        var before: Set<UInt64>
    }
    private var ownTurn: OwnTurn?
    /// Content height the runway holds (nil: the frame's own height).
    private var runwayHeight: CGFloat?
    /// Desktop OWN_SEND_GLIDE_RETAIN: 15% of the remaining glide per 60 fps
    /// frame (~90% in ~230 ms), ease-out.
    private static let glideRetain: Double = 0.85

    /// User rows near the tail right now (a new one after this is ours).
    private func recentUserKeys() -> Set<UInt64> {
        var keys = Set<UInt64>()
        if let frame = current {
            let n = frame.rowCount()
            for i in stride(from: Int(n) - 1, through: max(0, Int(n) - 24), by: -1) {
                if let p = frame.placement(index: UInt32(i)), p.kind == .user { keys.insert(p.key) }
            }
        }
        return keys
    }

    /// Reserve the reply's space below the prompt about to be sent (every
    /// immediate send; queued sends keep the live turn's runway).
    func beginOwnTurn() {
        queuedTurn = nil
        ownTurn = OwnTurn(key: nil, before: recentUserKeys())
        setFollowing(true)
    }

    /// A message queued behind the live turn: it gets the runway once its
    /// bubble materializes in the transcript (desktop
    /// `promote_materialized_queued_turn`), not before — the live turn keeps
    /// its own until then.
    func expectQueuedTurn() {
        if queuedTurn == nil { queuedTurn = recentUserKeys() }
    }
    private var queuedTurn: Set<UInt64>?

    /// Resolve the prompt row and the height the runway holds for `frame`.
    private func contentHeight(for frame: LayoutFrame) -> CGFloat {
        let natural = CGFloat(frame.totalHeight())
        if let before = queuedTurn {
            let n = Int(frame.rowCount())
            for i in stride(from: n - 1, through: max(0, n - 12), by: -1) {
                if let p = frame.placement(index: UInt32(i)), p.kind == .user, !before.contains(p.key) {
                    // The queued prompt landed: promote it to own the runway.
                    queuedTurn = nil
                    ownTurn = OwnTurn(key: p.key, before: before)
                    setFollowing(true)
                    break
                }
            }
        }
        guard var turn = ownTurn else {
            runwayHeight = nil
            return natural
        }
        if turn.key == nil {
            let n = Int(frame.rowCount())
            for i in stride(from: n - 1, through: max(0, n - 12), by: -1) {
                if let p = frame.placement(index: UInt32(i)), p.kind == .user, !turn.before.contains(p.key) {
                    turn.key = p.key
                    break
                }
            }
            ownTurn = turn
        }
        guard let key = turn.key, let i = frame.indexOf(key: key), let p = frame.placement(index: i) else {
            // Echo not laid out yet: hold still (desktop returns early too).
            return max(natural, runwayHeight ?? 0)
        }
        // Hold offset puts the prompt row (its turn gap included) at the top of
        // the visible area; the reservation makes that the max offset.
        let hold = CGFloat(p.y) - adjustedContentInset.top
        let minimum = hold + bounds.height - adjustedContentInset.bottom
        if natural >= minimum - 0.5 {
            // Filled: the ordinary tail follow takes over. The anchor stays for
            // the turn, so if the viewport grows past the reply again (the
            // keyboard going away) the space is reserved again instead of the
            // prompt dropping back down.
            runwayHeight = nil
            return natural
        }
        runwayHeight = minimum
        return minimum
    }

    /// The composer's inset changes with the keyboard: resize the runway in
    /// the same pass, so "the bottom" stays the held position (otherwise the
    /// prompt dipped and glided back).
    override var contentInset: UIEdgeInsets {
        didSet { if contentInset != oldValue { refreshRunway() } }
    }

    /// Insets / viewport changed (keyboard, rotation): re-size the runway.
    private func refreshRunway() {
        guard ownTurn != nil, let frame = current else { return }
        let height = contentHeight(for: frame)
        if abs(contentSize.height - height) > 0.5 {
            contentSize = CGSize(width: layoutWidth, height: height)
            if following, !isTracking { startSpring() }
        }
    }

    /// A disclosure the user just toggled: its next height change tweens with
    /// the desktop fold (140 ms ease-out).
    private var pendingFold: UInt64?
    /// Row frames animate during this apply (fold / tool-row arrival).
    private var frameAnimation: UIViewPropertyAnimator?

    private var spring: CADisplayLink?
    private var lastSpringTick: CFTimeInterval = 0
    private var viewportWidth: CGFloat = 0
    private var textScale: CGFloat = 1

    private struct ModelKey: Hashable {
        let key: UInt64
        let version: UInt64
        let width: Float
    }

    init(engine: TranscriptView) {
        self.engine = engine
        super.init(frame: .zero)
        alwaysBounceVertical = true
        keyboardDismissMode = .interactive
        contentInsetAdjustmentBehavior = .always
        showsVerticalScrollIndicator = true
        backgroundColor = .clear
        let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
        tap.cancelsTouchesInView = false
        addGestureRecognizer(tap)
        addGestureRecognizer(UILongPressGestureRecognizer(target: self, action: #selector(longPressed(_:))))
        addInteraction(editMenu)
        for handle in [startHandle, endHandle] {
            handle.addGestureRecognizer(UIPanGestureRecognizer(target: self, action: #selector(draggedHandle(_:))))
            addSubview(handle)
        }
        panGestureRecognizer.addTarget(self, action: #selector(panned(_:)))
        delegate = self
        // Display models are a pure cache (rebuilt from the frame on demand).
        memoryObserver = NotificationCenter.default.addObserver(forName: UIApplication.didReceiveMemoryWarningNotification, object: nil, queue: .main) { [weak self] _ in
            self?.cache.removeAll()
            self?.cacheOrder.removeAll()
            self?.pool.forEach { $0.removeFromSuperview() }
            self?.pool.removeAll()
        }
        // Rebuilt rows get new versions, so stale models just stop matching.
        MathSettings.apply(to: engine)
        mathObserver = NotificationCenter.default.addObserver(forName: MathSettings.didChange, object: nil, queue: .main) { [weak self] _ in
            guard let self else { return }
            MathSettings.apply(to: self.engine)
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    // MARK: Viewport

    /// The width between the side insets. They needn't match (landscape,
    /// the iPhone Duo's camera edge); the adjusted inset already shifts the
    /// content past the leading one, so rows span only what's left.
    private var layoutWidth: CGFloat {
        max(0, bounds.width - adjustedContentInset.left - adjustedContentInset.right)
    }

    override func adjustedContentInsetDidChange() {
        super.adjustedContentInsetDidChange()
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let scale = UIFontMetrics(forTextStyle: .body).scaledValue(for: 17) / 17
        if layoutWidth != viewportWidth || scale != textScale {
            viewportWidth = layoutWidth
            textScale = scale
            engine.setViewport(width: Float(layoutWidth), textScale: Float(scale))
        }
        layoutRows()
        refreshRunway()
        reportDistance()
    }

    var maxOffsetY: CGFloat {
        max(-adjustedContentInset.top, contentSize.height + adjustedContentInset.bottom - bounds.height)
    }

    var distanceFromBottom: CGFloat { maxOffsetY - contentOffset.y }

    private var lastReportedY: CGFloat = 0

    /// Follow re-engages only when the user *returns* to the tail: momentum
    /// carrying the list down into the last 70pt. Never while the finger is
    /// down — a drag that starts at the bottom would otherwise re-latch
    /// immediately and spring back down on release.
    private func reportDistance() {
        let distance = distanceFromBottom
        onDistanceFromBottom?(distance)
        let movingDown = contentOffset.y > lastReportedY + 0.1
        lastReportedY = contentOffset.y
        if !following, !isTracking, isDecelerating, movingDown || distance <= 0, distance < 70 {
            setFollowing(true)
        }
    }

    private func setFollowing(_ value: Bool) {
        guard following != value else { return }
        following = value
        onFollowChange?(value)
        if !value { stopSpring() }
    }

    /// A drag hands control to the user. Releasing within 70pt of the tail
    /// while not flinging away from it hands control back.
    @objc private func panned(_ pan: UIPanGestureRecognizer) {
        switch pan.state {
        case .began:
            setFollowing(false)
            editMenu.dismissMenu()
        case .ended, .cancelled:
            // Finger moving up (negative y) scrolls toward the tail.
            if distanceFromBottom < 70, pan.velocity(in: self).y <= 50 { setFollowing(true) }
            if !isDecelerating { showSelectionMenu() }
        default:
            break
        }
    }

    func scrollViewDidEndDecelerating(_ scrollView: UIScrollView) { showSelectionMenu() }

    /// Status-bar tap: the user is going to the top, not the tail.
    func scrollViewShouldScrollToTop(_ scrollView: UIScrollView) -> Bool {
        setFollowing(false)
        return true
    }

    /// Stop following the tail (programmatic scrolling, benchmarks).
    func releaseFollow() { setFollowing(false) }

    /// Jump (or glide) to the newest content and resume following.
    func scrollToBottom(animated: Bool) {
        setFollowing(true)
        if animated {
            startSpring()
        } else {
            stopSpring()
            contentOffset.y = maxOffsetY
        }
    }

    /// The system scroll-edge effect only engages after an on-screen scroll;
    /// the first frame positions us mid-transition, so settle once visible.
    func settleEdgeEffect() {
        guard current != nil else { return }
        let target = following ? maxOffsetY : contentOffset.y
        contentOffset.y = target - 1
        contentOffset.y = target
    }

    // MARK: Frames

    func apply(_ frame: LayoutFrame) {
        let state = Signposts.transcript.beginInterval("apply-frame")
        let t0 = CACurrentMediaTime()
        defer {
            Signposts.transcript.endInterval("apply-frame", state)
            TranscriptPerf.maxApplyMs = max(TranscriptPerf.maxApplyMs, (CACurrentMediaTime() - t0) * 1000)
        }
        if frame.styleCount() != fonts.count { fonts.update(frame.styles()) }
        let first = current == nil || current?.rowCount() == 0
        var anchor: (key: UInt64, delta: CGFloat)?
        if !following, let old = current {
            let y = contentOffset.y + adjustedContentInset.top
            if let i = old.indexAt(y: Float(max(0, y))), let p = old.placement(index: i) {
                anchor = (p.key, contentOffset.y - CGFloat(p.y))
            }
        }
        let old = current
        current = frame
        if old == nil || old?.rowCount() == 0 {
            // Groups present when the transcript attaches never animate.
            ToolRailView.quietUntil = CACurrentMediaTime() + 0.8
        }
        let animation = rowAnimation(old: old, new: frame)
        let height = contentHeight(for: frame)
        if contentSize.height != height || contentSize.width != layoutWidth {
            contentSize = CGSize(width: layoutWidth, height: height)
        }
        if first {
            for i in 0..<frame.rowCount() { if let p = frame.placement(index: i) { knownKeys.insert(p.key) } }
            contentOffset.y = maxOffsetY
        } else if following {
            // A reflow for a new width (rotation, iPhone Duo folding) lands
            // on the tail at once; the follow spring visibly glided there
            // under the system's fold animation.
            if let old, old.width() != frame.width() {
                stopSpring()
                contentOffset.y = maxOffsetY
            } else if !isTracking {
                startSpring()
            }
        } else if let anchor, let i = frame.indexOf(key: anchor.key), let p = frame.placement(index: i) {
            let target = CGFloat(p.y) + anchor.delta
            if abs(target - contentOffset.y) > 0.5 {
                // Shift bounds (not contentOffset) so momentum survives.
                bounds.origin.y = target
            }
        }
        layoutRows(animation: animation)
    }

    /// Fold toggles tween 140 ms ease-out; a visible tool group that grew
    /// (a new call arrived) reveals over 360 ms expo — desktop timings.
    private func rowAnimation(old: LayoutFrame?, new: LayoutFrame) -> UIViewPropertyAnimator? {
        guard let old, !UIAccessibility.isReduceMotionEnabled else { return nil }
        func height(_ f: LayoutFrame, _ key: UInt64) -> Float? {
            f.indexOf(key: key).flatMap { f.placement(index: $0)?.height }
        }
        if let key = pendingFold, let a = height(old, key), let b = height(new, key), a != b {
            pendingFold = nil
            return UIViewPropertyAnimator(duration: Motion.fold, curve: .easeOut)
        }
        let grew = visible.contains { key, view in
            view.kind == .tools && (height(new, key) ?? 0) > (height(old, key) ?? .greatestFiniteMagnitude)
        }
        return grew ? UIViewPropertyAnimator(duration: Motion.rowReveal, timingParameters: Motion.expo) : nil
    }

    private func layoutRows(animation: UIViewPropertyAnimator? = nil) {
        guard let frame = current else { return }
        var moves: [(RowView, CGRect)] = []
        let state = Signposts.transcript.beginInterval("layout-rows")
        let t0 = CACurrentMediaTime()
        defer {
            Signposts.transcript.endInterval("layout-rows", state)
            TranscriptPerf.maxLayoutMs = max(TranscriptPerf.maxLayoutMs, (CACurrentMediaTime() - t0) * 1000)
        }
        let y0 = contentOffset.y - overscan
        let y1 = contentOffset.y + bounds.height + overscan
        let placements = frame.rowsIn(y0: Float(y0), y1: Float(y1))
        var seen = Set<UInt64>(minimumCapacity: placements.count)
        let width = frame.width()
        for p in placements {
            seen.insert(p.key)
            let view: RowView
            if let v = visible[p.key] {
                view = v
            } else {
                view = pool.popLast() ?? { TranscriptPerf.viewsCreated += 1; return RowView() }()
                view.delegate = self
                visible[p.key] = view
                if view.superview !== self { addSubview(view) }
                view.isHidden = false
            }
            let stale = view.model == nil || view.version != p.version || view.model?.display.width != width || view.key != p.key
            if stale, view.key == p.key, view.model?.display.width == width,
               cache[ModelKey(key: p.key, version: p.version, width: width)] == nil {
                // Same row, new content (a streaming tail): keep painting the
                // current model and build the new one off the main thread.
                upgradeAsync(p, frame: frame)
            } else if stale {
                view.configure(model(for: p, frame: frame), kind: p.kind)
                view.uploadProgress = uploadProgress
                if !knownKeys.contains(p.key) {
                    knownKeys.insert(p.key)
                    view.alpha = 0
                    UIView.animate(withDuration: 0.28, delay: 0, options: [.curveEaseOut, .allowUserInteraction]) { view.alpha = 1 }
                }
            }
            let rect = CGRect(x: 0, y: CGFloat(p.y), width: layoutWidth, height: CGFloat(p.height))
            if view.frame != rect {
                // Existing rows tween with the animation; new ones just land.
                if animation != nil, !view.frame.isEmpty, !stale || view.key == p.key {
                    moves.append((view, rect))
                } else {
                    view.frame = rect
                }
            }
        }
        for (key, view) in visible where !seen.contains(key) {
            visible[key] = nil
            view.setSelection(nil)
            view.isHidden = true
            view.layer.removeAllAnimations()
            view.alpha = 1
            pool.append(view)
        }
        if let animation, !moves.isEmpty {
            for (view, _) in moves where view.kind == .tools { view.clipsToBounds = true }
            animation.addAnimations { for (view, rect) in moves { view.frame = rect } }
            animation.addCompletion { _ in for (view, _) in moves { view.clipsToBounds = false } }
            animation.startAnimation()
        } else {
            for (view, rect) in moves { view.frame = rect }
        }
        if selAnchor != nil { updateSelection() }
        prefetch(frame: frame, around: y0...y1)
    }

    private func model(for p: RowPlacement, frame: LayoutFrame) -> RowModel {
        let k = ModelKey(key: p.key, version: p.version, width: frame.width())
        if let m = cache[k] { return m }
        let state = Signposts.transcript.beginInterval("build-model-sync")
        let t0 = CACurrentMediaTime()
        TranscriptPerf.syncBuilds += 1
        defer {
            Signposts.transcript.endInterval("build-model-sync", state)
            TranscriptPerf.maxSyncBuildMs = max(TranscriptPerf.maxSyncBuildMs, (CACurrentMediaTime() - t0) * 1000)
        }
        let display = frame.display(index: p.index)!
        let m = RowModel(display: display, fonts: fonts)
        store(m, for: k)
        return m
    }

    private func store(_ m: RowModel, for k: ModelKey) {
        if cache[k] == nil { cacheOrder.append(k) }
        cache[k] = m
        if cacheOrder.count > 500 {
            for old in cacheOrder.prefix(100) { cache[old] = nil }
            cacheOrder.removeFirst(100)
        }
    }

    private func upgradeAsync(_ p: RowPlacement, frame: LayoutFrame) {
        let k = ModelKey(key: p.key, version: p.version, width: frame.width())
        guard !inflight.contains(k) else { return }
        inflight.insert(k)
        let fonts = self.fonts
        prefetchQueue.async { [weak self] in
            guard let d = frame.display(index: p.index) else { return }
            let model = RowModel(display: d, fonts: fonts)
            TranscriptPerf.asyncBuilds += 1
            DispatchQueue.main.async {
                guard let self else { return }
                self.inflight.remove(k)
                self.store(model, for: k)
                // Apply only if this is still the newest version on screen.
                if let view = self.visible[p.key], let current = self.current,
                   let i = current.indexOf(key: p.key), current.placement(index: i)?.version == p.version {
                    view.configure(model, kind: p.kind)
                    view.uploadProgress = self.uploadProgress
                    if self.selAnchor != nil { self.updateSelection() }
                }
            }
        }
    }

    /// Build display models for rows just beyond the realized band off-main,
    /// in the direction of travel first.
    private func prefetch(frame: LayoutFrame, around band: ClosedRange<CGFloat>) {
        let ahead = bounds.height * 1.5
        let down = panGestureRecognizer.velocity(in: self).y <= 0
        let ranges = down
            ? [(band.upperBound, band.upperBound + ahead), (band.lowerBound - ahead / 2, band.lowerBound)]
            : [(band.lowerBound - ahead, band.lowerBound), (band.upperBound, band.upperBound + ahead / 2)]
        var todo: [RowPlacement] = []
        for (a, b) in ranges where b > 0 {
            for p in frame.rowsIn(y0: Float(a), y1: Float(b)) {
                let k = ModelKey(key: p.key, version: p.version, width: frame.width())
                if cache[k] == nil, !inflight.contains(k) {
                    inflight.insert(k)
                    todo.append(p)
                }
            }
        }
        guard !todo.isEmpty else { return }
        let fonts = self.fonts
        prefetchQueue.async { [weak self] in
            let built = todo.compactMap { p -> (ModelKey, RowModel)? in
                guard let d = frame.display(index: p.index) else { return nil }
                return (ModelKey(key: p.key, version: p.version, width: frame.width()), RowModel(display: d, fonts: fonts))
            }
            DispatchQueue.main.async {
                guard let self else { return }
                for (k, m) in built {
                    self.inflight.remove(k)
                    self.store(m, for: k)
                }
            }
        }
    }

    // MARK: Follow spring

    private func startSpring() {
        guard spring == nil else { return }
        let link = CADisplayLink(target: self, selector: #selector(springTick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 60, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        lastSpringTick = CACurrentMediaTime()
        spring = link
    }

    private func stopSpring() {
        spring?.invalidate()
        spring = nil
    }

    @objc private func springTick(_ link: CADisplayLink) {
        let now = link.targetTimestamp
        let dt = min(1 / 30, max(0, now - lastSpringTick))
        lastSpringTick = now
        guard following, !isTracking else { return stopSpring() }
        let target = maxOffsetY
        let delta = target - contentOffset.y
        if abs(delta) < 0.5 {
            contentOffset.y = target
            return stopSpring()
        }
        if runwayHeight != nil {
            // Runway glide: the desktop's frame-rate-independent ease-out.
            let frames = min(8, dt / (1.0 / 60))
            contentOffset.y += delta * CGFloat(1 - pow(Self.glideRetain, frames))
        } else {
            // Critically damped approach: feels like the tail "settles" into place.
            contentOffset.y += delta * CGFloat(1 - exp(-dt * 16))
        }
    }

    /// The first user message bubble currently laid out (handoff target).
    func firstUserBubble(in target: UIView) -> CGRect? {
        let rows = visible.values.filter { $0.kind == .user && !$0.isHidden }.sorted { $0.frame.minY < $1.frame.minY }
        for row in rows {
            guard let d = row.model?.display,
                  let box = d.boxes.first(where: { $0.color == .userBubble && $0.scroller == nil })
            else { continue }
            let r = CGRect(x: CGFloat(box.x), y: CGFloat(box.y), width: CGFloat(box.w), height: CGFloat(box.h))
            return row.convert(r, to: target)
        }
        return nil
    }

    // MARK: Interaction

    @objc private func tapped(_ tap: UITapGestureRecognizer) {
        let point = tap.location(in: self)
        if selAnchor != nil {
            if !startHandle.hitFrame.contains(point), !endHandle.hitFrame.contains(point) { clearSelection() }
            return
        }
        guard let row = visible.values.first(where: { !$0.isHidden && $0.frame.contains(point) }),
              let url = row.link(at: tap.location(in: row))
        else { return }
        rowView(row, open: url)
    }

    func rowView(_ view: RowView, toggle key: UInt64) {
        UISelectionFeedbackGenerator().selectionChanged()
        pendingFold = key
        engine.toggle(key: key)
    }

    func rowView(_ view: RowView, toggleDetail detail: UInt64, open: Bool) {
        pendingFold = view.key
        engine.toggleDetail(row: view.key, detail: detail, open: open)
    }

    func rowView(_ view: RowView, perform action: String) {
        onAction?(action)
    }

    func rowView(_ view: RowView, openSubagent docId: String, title: String, running: Bool) {
        onSubagent?(docId, title, running)
    }

    func rowView(_ view: RowView, open url: URL) {
        guard let vc = findViewController() else { return }
        // An image chip opens its upload full size, as its thumbnail did.
        if url.scheme == "commutator-preview" {
            guard let reference = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                .queryItems?.first(where: { $0.name == "ref" })?.value
            else { return }
            Task { @MainActor [weak self, weak view, weak vc] in
                guard let image = await self?.imageFetcher?(reference), let view, let vc else { return }
                vc.present(ImageViewer(image: image, source: view), animated: true)
            }
            return
        }
        // A sender caption opens the chat that sent the message.
        if url.scheme == "commutator-chat" {
            guard let id = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                .queryItems?.first(where: { $0.name == "id" })?.value
            else { return }
            onOpenChat?(id)
            return
        }
        if url.scheme == "http" || url.scheme == "https" {
            let safari = SFSafariViewController(url: url)
            safari.preferredControlTintColor = Palette.accent
            vc.present(safari, animated: true)
        } else {
            UIApplication.shared.open(url)
        }
    }

    func rowView(_ view: RowView, imageFor reference: String, into imageView: UIImageView) {
        imageLoader?(reference, imageView)
    }

    func rowView(_ view: RowView, visualFor id: String) -> UIView? {
        if let live = visuals[id] { return live }
        guard let session = engine.visual(id: id) else { return nil }
        let live = VisualView(session: session, scale: traitCollection.displayScale)
        live.openURL = { [weak self, weak live] url in
            guard let self, let row = live?.superview as? RowView else { return }
            self.rowView(row, open: url)
        }
        visuals[id] = live
        return live
    }

    // MARK: Text selection
    //
    // The transcript is painted, so selection is ours: a long press selects
    // the word under the finger and keeps extending while the finger moves;
    // handles adjust either end, across rows; a tap clears.

    private struct TextPoint {
        var key: UInt64
        var offset: Int
    }
    private var selAnchor: TextPoint?
    private var selFocus: TextPoint?
    /// The long-pressed word, while that press extends the selection.
    private var pressWord: (key: UInt64, range: Range<Int>)?
    /// Finger → caret offset of the handle being dragged.
    private var grab: CGPoint = .zero
    private let startHandle = SelectionHandle(start: true)
    private let endHandle = SelectionHandle(start: false)
    private lazy var editMenu = UIEditMenuInteraction(delegate: self)

    private func index(_ p: TextPoint) -> UInt32? { current?.indexOf(key: p.key) }

    /// a < b in reading order (nil when either row is gone).
    private func precedes(_ a: TextPoint, _ b: TextPoint) -> Bool? {
        guard let ia = index(a), let ib = index(b) else { return nil }
        return (ia, a.offset) < (ib, b.offset)
    }

    /// The selection's ends in reading order, with their row indices.
    private func ordered() -> (lo: TextPoint, loIndex: UInt32, hi: TextPoint, hiIndex: UInt32)? {
        guard let a = selAnchor, let f = selFocus, let ia = index(a), let iff = index(f) else { return nil }
        return (ia, a.offset) <= (iff, f.offset) ? (a, ia, f, iff) : (f, iff, a, ia)
    }

    private func clearSelection() {
        selAnchor = nil
        selFocus = nil
        pressWord = nil
        editMenu.dismissMenu()
        startHandle.isHidden = true
        endHandle.isHidden = true
        visible.values.forEach { $0.setSelection(nil) }
    }

    private func updateSelection() {
        guard let o = ordered() else { return clearSelection() }
        for view in visible.values where !view.isHidden {
            guard let i = current?.indexOf(key: view.key), i >= o.loIndex, i <= o.hiIndex else {
                view.setSelection(nil)
                continue
            }
            view.setSelection((i == o.loIndex ? o.lo.offset : 0)..<(i == o.hiIndex ? o.hi.offset : Int.max))
        }
        for (handle, point, end) in [(startHandle, o.lo, false), (endHandle, o.hi, true)] {
            if let row = visible[point.key], !row.isHidden, let caret = row.caret(at: point.offset, end: end) {
                handle.place(caret: row.convert(caret, to: self))
                handle.isHidden = false
                bringSubviewToFront(handle)
            } else {
                handle.isHidden = true
            }
        }
    }

    /// The text position nearest `point` (list coordinates) within `limit`.
    private func hitText(_ point: CGPoint, within limit: CGFloat = .infinity) -> TextPoint? {
        var best: (point: TextPoint, distance: CGFloat)?
        for view in visible.values where !view.isHidden {
            let rowDistance = max(0, view.frame.minY - point.y, point.y - view.frame.maxY)
            if let b = best, rowDistance >= b.distance { continue }
            guard let hit = view.textOffset(at: convert(point, to: view)) else { continue }
            if best == nil || hit.distance < best!.distance {
                best = (TextPoint(key: view.key, offset: hit.offset), hit.distance)
            }
        }
        return best.flatMap { $0.distance <= limit ? $0.point : nil }
    }

    private func selectedText() -> String {
        guard let o = ordered(), let frame = current else { return "" }
        var parts: [String] = []
        for i in o.loIndex...o.hiIndex {
            let display: RowDisplay?
            if let p = frame.placement(index: i), let v = visible[p.key], let m = v.model { display = m.display } else { display = frame.display(index: i) }
            guard let display else { continue }
            let text = display.selectedText(from: i == o.loIndex ? o.lo.offset : 0, to: i == o.hiIndex ? o.hi.offset : Int.max)
            if !text.isEmpty { parts.append(text) }
        }
        return parts.joined(separator: "\n\n")
    }

    @objc private func longPressed(_ g: UILongPressGestureRecognizer) {
        let point = g.location(in: self)
        switch g.state {
        case .began:
            guard let hit = hitText(point, within: 24), let display = visible[hit.key]?.model?.display else { return }
            let word = display.wordRange(at: hit.offset)
            pressWord = (hit.key, word)
            selAnchor = TextPoint(key: hit.key, offset: word.lowerBound)
            selFocus = TextPoint(key: hit.key, offset: word.upperBound)
            editMenu.dismissMenu()
            UIImpactFeedbackGenerator(style: .medium).impactOccurred()
            updateSelection()
        case .changed:
            guard let word = pressWord, let hit = hitText(point) else { return }
            let start = TextPoint(key: word.key, offset: word.range.lowerBound)
            let end = TextPoint(key: word.key, offset: word.range.upperBound)
            // Before the word: grow back from its end; else forward from its start.
            if precedes(hit, start) == true {
                selAnchor = end
                selFocus = hit
            } else {
                selAnchor = start
                selFocus = precedes(hit, end) == true ? end : hit
            }
            updateSelection()
        case .ended, .cancelled:
            guard pressWord != nil else { return }
            pressWord = nil
            showSelectionMenu()
        default:
            break
        }
    }

    @objc private func draggedHandle(_ g: UIPanGestureRecognizer) {
        guard let handle = g.view as? SelectionHandle, let o = ordered() else { return }
        let start = handle === startHandle
        switch g.state {
        case .began:
            editMenu.dismissMenu()
            selAnchor = start ? o.hi : o.lo
            selFocus = start ? o.lo : o.hi
            let touch = g.location(in: self)
            grab = CGPoint(x: handle.caret.midX - touch.x, y: handle.caret.midY - touch.y)
        case .changed:
            let touch = g.location(in: self)
            guard let anchor = selAnchor,
                  let hit = hitText(CGPoint(x: touch.x + grab.x, y: touch.y + grab.y)),
                  // The ends don't cross (the handles would swap under the finger).
                  (start ? precedes(hit, anchor) : precedes(anchor, hit)) == true
            else { return }
            if hit.key != selFocus?.key || hit.offset != selFocus?.offset {
                selFocus = hit
                UISelectionFeedbackGenerator().selectionChanged()
                updateSelection()
            }
        case .ended, .cancelled:
            showSelectionMenu()
        default:
            break
        }
    }

    /// No scrolling while a press extends the selection or a handle moves.
    override func gestureRecognizerShouldBegin(_ g: UIGestureRecognizer) -> Bool {
        if g === panGestureRecognizer {
            if pressWord != nil { return false }
            let p = g.location(in: self)
            if startHandle.hitFrame.insetBy(dx: -12, dy: -12).contains(p) || endHandle.hitFrame.insetBy(dx: -12, dy: -12).contains(p) { return false }
        }
        return super.gestureRecognizerShouldBegin(g)
    }

    private var selectionRect: CGRect? {
        let rects = [startHandle, endHandle].filter { !$0.isHidden }.map(\.caret)
        guard let first = rects.first else { return nil }
        return rects.dropFirst().reduce(first) { $0.union($1) }
    }

    private func showSelectionMenu() {
        guard selAnchor != nil, pressWord == nil, let rect = selectionRect else { return }
        editMenu.presentEditMenu(with: UIEditMenuConfiguration(identifier: nil, sourcePoint: CGPoint(x: rect.midX, y: rect.minY)))
    }
}

extension TranscriptListView: UIEditMenuInteractionDelegate {
    func editMenuInteraction(_ interaction: UIEditMenuInteraction, menuFor configuration: UIEditMenuConfiguration, suggestedActions: [UIMenuElement]) -> UIMenu? {
        guard let o = ordered(), let frame = current else { return nil }
        var actions: [UIMenuElement] = [UIAction(title: "Copy") { [weak self] _ in
            guard let self else { return }
            UIPasteboard.general.string = self.selectedText()
            self.clearSelection()
        }]
        if let message = frame.messageText(index: o.loIndex), !message.isEmpty {
            actions.append(UIAction(title: "Copy Message") { [weak self] _ in
                UIPasteboard.general.string = message
                self?.clearSelection()
            })
        }
        return UIMenu(children: actions)
    }

    func editMenuInteraction(_ interaction: UIEditMenuInteraction, targetRectFor configuration: UIEditMenuConfiguration) -> CGRect {
        selectionRect ?? .null
    }
}

/// One end of a transcript selection: a caret bar with a knob above (start)
/// or below (end); the view is wider than it draws, to be easy to grab.
final class SelectionHandle: UIView {
    private let start: Bool
    /// The caret it marks, in the superview's coordinates.
    private(set) var caret: CGRect = .zero
    private static let knob: CGFloat = 10
    private static let width: CGFloat = 36

    init(start: Bool) {
        self.start = start
        super.init(frame: .zero)
        isOpaque = false
        backgroundColor = .clear
        isHidden = true
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Generous touch target around the drawn handle.
    var hitFrame: CGRect { isHidden ? .null : frame.insetBy(dx: 0, dy: -8) }

    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        bounds.insetBy(dx: 0, dy: -8).contains(point)
    }

    func place(caret: CGRect) {
        guard caret != self.caret || frame.isEmpty else { return }
        self.caret = caret
        let extra = Self.knob + 2
        frame = CGRect(x: caret.midX - Self.width / 2, y: caret.minY - (start ? extra : 0), width: Self.width, height: caret.height + extra)
        setNeedsDisplay()
    }

    override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext() else { return }
        ctx.setFillColor(Palette.accent.cgColor)
        let extra = Self.knob + 2
        let barTop = start ? extra : 0
        ctx.fill(CGRect(x: bounds.midX - 1, y: barTop, width: 2, height: caret.height))
        let knobY = start ? 0 : caret.height + 2
        ctx.fillEllipse(in: CGRect(x: bounds.midX - Self.knob / 2, y: knobY, width: Self.knob, height: Self.knob))
    }
}

extension UIView {
    func findViewController() -> UIViewController? {
        var r: UIResponder? = self
        while let next = r?.next {
            if let vc = next as? UIViewController { return vc }
            r = next
        }
        return nil
    }
}

/// Full-text selection for a message (the transcript itself is paint-only).
final class SelectTextViewController: UIViewController {
    private let text: String
    private let mono: Bool

    init(text: String, title: String = "Select Text", mono: Bool = false) {
        self.text = text
        self.mono = mono
        super.init(nibName: nil, bundle: nil)
        self.title = title
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        let tv = UITextView()
        tv.text = text
        tv.isEditable = false
        tv.font = mono ? Fonts.ui(.mono, 14) : Fonts.ui(.sans, UIFontMetrics(forTextStyle: .body).scaledValue(for: 16.5))
        tv.textColor = Palette.text
        tv.backgroundColor = .clear
        tv.textContainerInset = UIEdgeInsets(top: 16, left: 14, bottom: 32, right: 14)
        tv.frame = view.bounds
        tv.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(tv)
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        if !mono { DispatchQueue.main.async { tv.selectAll(nil) } }
    }
}
