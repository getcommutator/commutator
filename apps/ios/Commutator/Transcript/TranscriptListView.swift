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
        // System selection: UIKit's own long press, handles, loupe and edit
        // menu, driven through the UITextInput adapter below.
        textInteraction.textInput = self
        addInteraction(textInteraction)
        // The selection highlight takes the tint; resolve it per theme.
        tintColor = Palette.accent.resolvedColor(with: traitCollection)
        registerForTraitChanges(Palette.colorTraits) { (self: TranscriptListView, _) in
            self.tintColor = Palette.accent.resolvedColor(with: self.traitCollection)
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
        case .ended, .cancelled:
            // Finger moving up (negative y) scrolls toward the tail.
            if distanceFromBottom < 70, pan.velocity(in: self).y <= 50 { setFollowing(true) }
        default:
            break
        }
    }

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
        // Selection rects follow realized rows: re-ask UIKit when they change.
        var geometryChanged = false
        for p in placements {
            seen.insert(p.key)
            let view: RowView
            if let v = visible[p.key] {
                view = v
            } else {
                geometryChanged = true
                view = pool.popLast() ?? { TranscriptPerf.viewsCreated += 1; return RowView() }()
                view.delegate = self
                visible[p.key] = view
                // Rows stay below UIKit's selection highlight and handles.
                if view.superview !== self { insertSubview(view, at: 0) }
                view.isHidden = false
            }
            let stale = view.model == nil || view.version != p.version || view.model?.display.width != width || view.key != p.key
            if stale, view.key == p.key, view.model?.display.width == width,
               cache[ModelKey(key: p.key, version: p.version, width: width)] == nil {
                // Same row, new content (a streaming tail): keep painting the
                // current model and build the new one off the main thread.
                upgradeAsync(p, frame: frame)
            } else if stale {
                geometryChanged = true
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
                geometryChanged = true
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
            geometryChanged = true
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
        if selection != nil {
            if selection.map({ index($0.lo) == nil || index($0.hi) == nil }) == true {
                clearSelection()
            } else if geometryChanged {
                selectionGeometryChanged()
            }
        }
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
                    if self.selection != nil { self.selectionGeometryChanged() }
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
        // A tap over a selection only dismisses it (UITextInteraction does).
        guard selection == nil else { return }
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
    // The transcript is painted, so UIKit can't see its text: the list is a
    // non-editable `UITextInput` instead, and `UITextInteraction` brings the
    // system long press, handles, loupe and edit menu. Positions are
    // (row key, UTF-16 offset in the row's text); geometry comes from the
    // realized rows, so ends in rows scrolled out have no handle.

    private let textInteraction = UITextInteraction(for: .nonEditable)
    private var selection: (lo: TranscriptPosition, hi: TranscriptPosition)?
    weak var inputDelegate: UITextInputDelegate?
    private(set) lazy var tokenizer: UITextInputTokenizer = TranscriptTokenizer(list: self)

    private func index(_ p: TranscriptPosition) -> UInt32? { current?.indexOf(key: p.key) }

    private func order(_ p: TranscriptPosition) -> (UInt32, Int)? { index(p).map { ($0, p.offset) } }

    /// Row `i`'s display: the realized model when there is one.
    fileprivate func display(_ i: UInt32) -> RowDisplay? {
        guard let frame = current, let p = frame.placement(index: i) else { return nil }
        return visible[p.key]?.model?.display ?? frame.display(index: i)
    }

    fileprivate func display(key: UInt64) -> RowDisplay? {
        current?.indexOf(key: key).flatMap(display)
    }

    private func length(_ i: UInt32) -> Int { display(i).map { ($0.text as NSString).length } ?? 0 }

    private func position(_ i: UInt32, _ offset: Int) -> TranscriptPosition? {
        current?.placement(index: i).map { TranscriptPosition(key: $0.key, offset: offset) }
    }

    private func clearSelection() {
        guard selection != nil else { return }
        selection = nil
        selectionGeometryChanged()
    }

    /// Rows moved or were realized: UIKit re-asks for the selection rects.
    private func selectionGeometryChanged() {
        inputDelegate?.selectionWillChange(self)
        inputDelegate?.selectionDidChange(self)
    }

    /// The text position nearest `point` (list coordinates) within `limit`.
    private func hitText(_ point: CGPoint, within limit: CGFloat = .infinity) -> TranscriptPosition? {
        var best: (point: TranscriptPosition, distance: CGFloat)?
        for view in visible.values where !view.isHidden {
            let rowDistance = max(0, view.frame.minY - point.y, point.y - view.frame.maxY)
            if let b = best, rowDistance >= b.distance { continue }
            guard let hit = view.textOffset(at: convert(point, to: view)) else { continue }
            if best == nil || hit.distance < best!.distance {
                best = (TranscriptPosition(key: view.key, offset: hit.offset), hit.distance)
            }
        }
        return best.flatMap { $0.distance <= limit ? $0.point : nil }
    }

    private func text(from lo: TranscriptPosition, to hi: TranscriptPosition) -> String {
        guard let li = index(lo), let hiIndex = index(hi), li <= hiIndex else { return "" }
        var parts: [String] = []
        for i in li...hiIndex {
            guard let display = display(i) else { continue }
            let text = display.selectedText(from: i == li ? lo.offset : 0, to: i == hiIndex ? hi.offset : Int.max)
            if !text.isEmpty { parts.append(text) }
        }
        return parts.joined(separator: "\n\n")
    }

    // MARK: Responder (copy from the system edit menu)

    override var canBecomeFirstResponder: Bool { true }

    override func resignFirstResponder() -> Bool {
        let resigned = super.resignFirstResponder()
        if resigned { clearSelection() }
        return resigned
    }

    override func canPerformAction(_ action: Selector, withSender sender: Any?) -> Bool {
        if action == #selector(copy(_:)) { return selection.map { $0.lo != $0.hi } ?? false }
        return super.canPerformAction(action, withSender: sender)
    }

    override func copy(_ sender: Any?) {
        guard let s = selection else { return }
        UIPasteboard.general.string = text(from: s.lo, to: s.hi)
        clearSelection()
    }
}

/// A place in the transcript: a row and a UTF-16 offset in its text.
final class TranscriptPosition: UITextPosition {
    let key: UInt64
    let offset: Int

    init(key: UInt64, offset: Int) {
        self.key = key
        self.offset = offset
    }

    static func == (a: TranscriptPosition, b: TranscriptPosition) -> Bool { a.key == b.key && a.offset == b.offset }
    static func != (a: TranscriptPosition, b: TranscriptPosition) -> Bool { !(a == b) }
}

final class TranscriptRange: UITextRange {
    let lo: TranscriptPosition
    let hi: TranscriptPosition

    init(_ lo: TranscriptPosition, _ hi: TranscriptPosition) {
        self.lo = lo
        self.hi = hi
    }

    override var start: UITextPosition { lo }
    override var end: UITextPosition { hi }
    override var isEmpty: Bool { lo == hi }
}

final class TranscriptSelectionRect: UITextSelectionRect {
    private let _rect: CGRect
    private let _start: Bool
    private let _end: Bool

    init(_ rect: CGRect, start: Bool, end: Bool) {
        _rect = rect
        _start = start
        _end = end
    }

    override var rect: CGRect { _rect }
    override var writingDirection: NSWritingDirection { .natural }
    override var containsStart: Bool { _start }
    override var containsEnd: Bool { _end }
    override var isVertical: Bool { false }
}

/// Words as the old long press picked them (`RowDisplay.wordRange`: within
/// one run, so edge words of back-to-back paragraphs don't merge).
final class TranscriptTokenizer: UITextInputStringTokenizer {
    private weak var list: TranscriptListView?

    init(list: TranscriptListView) {
        self.list = list
        super.init(textInput: list)
    }

    override func rangeEnclosingPosition(_ position: UITextPosition, with granularity: UITextGranularity, inDirection direction: UITextDirection) -> UITextRange? {
        if granularity == .word, let p = position as? TranscriptPosition, let display = list?.display(key: p.key) {
            let word = display.wordRange(at: p.offset)
            return TranscriptRange(TranscriptPosition(key: p.key, offset: word.lowerBound), TranscriptPosition(key: p.key, offset: word.upperBound))
        }
        return super.rangeEnclosingPosition(position, with: granularity, inDirection: direction)
    }
}

extension TranscriptListView: UITextInput {
    // MARK: Document and positions

    var beginningOfDocument: UITextPosition {
        position(0, 0) ?? TranscriptPosition(key: 0, offset: 0)
    }

    var endOfDocument: UITextPosition {
        guard let n = current?.rowCount(), n > 0 else { return beginningOfDocument }
        return position(n - 1, length(n - 1)) ?? beginningOfDocument
    }

    func textRange(from fromPosition: UITextPosition, to toPosition: UITextPosition) -> UITextRange? {
        guard let a = fromPosition as? TranscriptPosition, let b = toPosition as? TranscriptPosition,
              let oa = order(a), let ob = order(b) else { return nil }
        return oa <= ob ? TranscriptRange(a, b) : TranscriptRange(b, a)
    }

    /// Rows join end to start: a row's end and the next row's start are the
    /// same place, `offset` 0 apart.
    func position(from position: UITextPosition, offset: Int) -> UITextPosition? {
        guard let p = position as? TranscriptPosition, let n = current?.rowCount(), var i = index(p) else { return nil }
        var o = p.offset + offset
        while true {
            if o < 0 {
                guard i > 0 else { return nil }
                i -= 1
                o += length(i)
            } else if o > length(i) {
                guard i + 1 < n else { return nil }
                o -= length(i)
                i += 1
            } else {
                return self.position(i, o)
            }
        }
    }

    // ponytail: up/down step by characters, not visual lines; nothing in a
    // non-editable selection moves by line.
    func position(from position: UITextPosition, in direction: UITextLayoutDirection, offset: Int) -> UITextPosition? {
        self.position(from: position, offset: direction == .left || direction == .up ? -offset : offset)
    }

    func compare(_ position: UITextPosition, to other: UITextPosition) -> ComparisonResult {
        guard let a = position as? TranscriptPosition, let b = other as? TranscriptPosition,
              let oa = order(a), let ob = order(b) else { return .orderedSame }
        return oa < ob ? .orderedAscending : oa > ob ? .orderedDescending : .orderedSame
    }

    // ponytail: sums the lengths of the rows between, O(rows); fine for a
    // selection, a running prefix sum if UIKit ever asks across the whole doc.
    func offset(from: UITextPosition, to toPosition: UITextPosition) -> Int {
        guard let a = from as? TranscriptPosition, let b = toPosition as? TranscriptPosition,
              let ia = index(a), let ib = index(b) else { return 0 }
        if ia == ib { return b.offset - a.offset }
        if ia > ib { return -offset(from: b, to: a) }
        var n = length(ia) - a.offset
        for i in (ia + 1)..<ib { n += length(i) }
        return n + b.offset
    }

    func position(within range: UITextRange, farthestIn direction: UITextLayoutDirection) -> UITextPosition? {
        direction == .left || direction == .up ? range.start : range.end
    }

    func characterRange(byExtending position: UITextPosition, in direction: UITextLayoutDirection) -> UITextRange? {
        let other = self.position(from: position, in: direction, offset: 1) ?? position
        return textRange(from: position, to: other)
    }

    // MARK: Text

    func text(in range: UITextRange) -> String? {
        guard let r = range as? TranscriptRange else { return nil }
        return text(from: r.lo, to: r.hi)
    }

    var selectedTextRange: UITextRange? {
        get { selection.map { TranscriptRange($0.lo, $0.hi) } }
        set {
            guard let r = newValue as? TranscriptRange, !r.isEmpty else {
                selection = nil
                return
            }
            selection = (r.lo, r.hi)
        }
    }

    /// The system menu, plus copying the whole message the selection starts in.
    func editMenu(for textRange: UITextRange, suggestedActions: [UIMenuElement]) -> UIMenu? {
        guard let r = textRange as? TranscriptRange, let i = index(r.lo),
              let message = current?.messageText(index: i), !message.isEmpty
        else { return UIMenu(children: suggestedActions) }
        return UIMenu(children: suggestedActions + [UIAction(title: "Copy Message") { [weak self] _ in
            UIPasteboard.general.string = message
            self?.clearSelection()
        }])
    }

    // MARK: Geometry (list coordinates)

    func caretRect(for position: UITextPosition) -> CGRect {
        guard let p = position as? TranscriptPosition, let row = visible[p.key], !row.isHidden,
              let caret = row.caret(at: p.offset) else { return .null }
        return row.convert(caret, to: self)
    }

    func firstRect(for range: UITextRange) -> CGRect {
        selectionRects(for: range).first?.rect ?? .null
    }

    func selectionRects(for range: UITextRange) -> [UITextSelectionRect] {
        guard let r = range as? TranscriptRange, let li = index(r.lo), let hiIndex = index(r.hi) else { return [] }
        var rects: [CGRect] = []
        var start = false, end = false
        for view in visible.values.sorted(by: { $0.frame.minY < $1.frame.minY }) where !view.isHidden {
            guard let i = current?.indexOf(key: view.key), i >= li, i <= hiIndex else { continue }
            let found = view.selectionRects((i == li ? r.lo.offset : 0)..<(i == hiIndex ? r.hi.offset : Int.max))
            guard !found.isEmpty else { continue }
            if i == li { start = true }
            if i == hiIndex { end = true }
            rects += found.map { view.convert($0, to: self) }
        }
        return rects.enumerated().map { n, rect in
            TranscriptSelectionRect(rect, start: start && n == 0, end: end && n == rects.count - 1)
        }
    }

    func closestPosition(to point: CGPoint) -> UITextPosition? { hitText(point) }

    func closestPosition(to point: CGPoint, within range: UITextRange) -> UITextPosition? {
        guard let p = hitText(point) else { return nil }
        if compare(p, to: range.start) == .orderedAscending { return range.start }
        if compare(p, to: range.end) == .orderedDescending { return range.end }
        return p
    }

    func characterRange(at point: CGPoint) -> UITextRange? {
        guard let p = hitText(point), let display = display(key: p.key) else { return nil }
        let ns = display.text as NSString
        guard ns.length > 0 else { return nil }
        let c = ns.rangeOfComposedCharacterSequence(at: max(0, min(ns.length - 1, p.offset)))
        return TranscriptRange(TranscriptPosition(key: p.key, offset: c.location), TranscriptPosition(key: p.key, offset: NSMaxRange(c)))
    }

    // MARK: Editing (non-editable: no-ops)

    var markedTextRange: UITextRange? { nil }
    var markedTextStyle: [NSAttributedString.Key: Any]? {
        get { nil }
        set {}
    }
    func setMarkedText(_ markedText: String?, selectedRange: NSRange) {}
    func unmarkText() {}
    func replace(_ range: UITextRange, withText text: String) {}
    func baseWritingDirection(for position: UITextPosition, in direction: UITextStorageDirection) -> NSWritingDirection { .natural }
    func setBaseWritingDirection(_ writingDirection: NSWritingDirection, for range: UITextRange) {}
    var hasText: Bool { (current?.rowCount() ?? 0) > 0 }
    func insertText(_ text: String) {}
    func deleteBackward() {}
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
