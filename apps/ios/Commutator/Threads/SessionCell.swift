import UIKit

/// What a session row shows. Built from the core's front-page snapshot.
struct SessionRowVM: Hashable {
    enum Status: Hashable {
        case idle
        case completed
        case working
        case awaiting
        case errored
        /// Activity's: woken from sleep and not looked at since. Set only
        /// while Activity is on (`ActivityList.decorate`).
        case woke
    }

    enum PR: Hashable {
        case open, draft, merged, closed
    }

    let id: String
    var title: String
    var projectName: String
    /// False for project-less sessions (the tile reads "H" for Home, like desktop).
    var hasProject: Bool
    var colorIndex: Int
    var harness: String?
    var branch: String?
    var pr: PR?
    var prNumber: UInt64?
    var status: Status
    var timeLabel: String
    var unseen: Bool
    var pinned: Bool
    var sendFailed: Bool
    /// Waiting on background work that will wake its agent (drawn as
    /// Working: `status` reads `.working`).
    var waiting = false
    /// …on a process its agent said it waits on: Stop Waiting is offered.
    var canStopWaiting = false
    /// Asleep until then (Activity's Asleep).
    var sleepUntil: Date?
    /// Woke from sleep then, and not seen since.
    var wokeAt: Date?
    /// Its last move (the recency sort key).
    var lastActivity = Date(timeIntervalSince1970: 0)
    /// Its project (`ProjectRef.id`): the tile wears the repository's artwork.
    var projectId: String? = nil
}

/// Folder row ("Pinned 2 ›", "P0 3 ›").
struct FolderRowVM: Hashable {
    let id: String
    var name: String
    var count: Int
    var symbol: String
    /// A worktree's folder on its main checkout's device (a new chat there runs in it).
    var path: String? = nil
}

/// Two-line session row, laid out by hand: fixed height, no Auto Layout
/// solving per cell, no text measurement beyond single-line labels. Status
/// and PR badge follow the desktop sidebar.
///
///   [mark]  Title of the session ··········· ⠿ Working
///           [P] project  branch ············· ⎇ 412
final class SessionCell: UICollectionViewListCell {
    static var height: CGFloat { (62 * TypeScale.factor).rounded() }

    private let harness = UIImageView()
    private let title = FadingLabel()
    private let meta = FadingLabel()
    private let time = UILabel()
    private let status = StatusGlyph()
    private let prBadge = PRBadgeView()
    private var vm: SessionRowVM?
    /// The session open beside the sidebar (iPad): a quiet fill.
    var isCurrent = false { didSet { if isCurrent != oldValue { setNeedsUpdateConfiguration() } } }
    /// The desktop sidebar's Compact: the title line alone. Set before `configure`.
    var compact = false
    /// The Projects page's rows (a 44 pt list cell).
    static var compactHeight: CGFloat { (44 * TypeScale.factor).rounded() }

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.textColor = Palette.text
        time.textAlignment = .right
        harness.contentMode = .scaleAspectFit
        harness.tintColor = Palette.text
        for v in [harness, title, meta, time, status, prBadge] as [UIView] { contentView.addSubview(v) }
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
        // The project tile is a rasterized attachment: redraw it in the new tone.
        registerForTraitChanges(Palette.colorTraits) { (cell: SessionCell, _) in
            if let vm = cell.vm { cell.configure(vm) }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : isCurrent ? Palette.rowActive : .clear
        bg.cornerRadius = 16
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }

    /// Fixed height: skip Auto Layout self-sizing entirely.
    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = compact ? Self.compactHeight : Self.height
        return attrs
    }

    /// Desktop precedence: a failed send beats everything; then the live
    /// state; unseen-and-finished reads "Done"; otherwise the time.
    static func corner(_ vm: SessionRowVM) -> (word: String, color: UIColor, glyph: StatusGlyph.Kind)? {
        if vm.sendFailed { return ("Failed", Palette.danger, .dot(Palette.danger)) }
        switch vm.status {
        case .working: return ("Working", StatusTone.working, .spinner)
        case .awaiting: return ("Input", StatusTone.question, .symbol("questionmark", StatusTone.question))
        case .errored: return ("Failed", StatusTone.failed, .dot(StatusTone.failed))
        case .completed: return vm.unseen ? ("Done", StatusTone.done, .check(StatusTone.done)) : nil
        case .woke: return ("Woke", StatusTone.input, .symbol("alarm", StatusTone.input))
        case .idle: return nil
        }
    }

    /// A group's glyph: its most pressing chat's (a question, then done and
    /// not seen, then working), nil when nothing in it is.
    static func urgentGlyph(_ list: [SessionRowVM]) -> StatusGlyph.Kind? {
        func rank(_ vm: SessionRowVM) -> Int? {
            switch vm.status {
            case .awaiting: 0
            case .completed: vm.unseen ? 1 : nil
            case .woke: 2
            case .working: 3
            case .errored: 4
            case .idle: nil
            }
        }
        let urgent = list.compactMap { vm in rank(vm).map { ($0, vm) } }.min { $0.0 < $1.0 }?.1
        return urgent.flatMap { corner($0)?.glyph }
    }

    func configure(_ vm: SessionRowVM) {
        self.vm = vm
        harness.image = BrandMarks.image(for: vm.harness ?? "claude-code", side: Self.markSide)
        title.text = vm.title
        // Compact matches the Projects page's rows (Agents): regular 15.
        title.font = compact ? Fonts.ui(.sans, TypeScale.size(15)) : Fonts.ui(vm.unseen ? .sansSemibold : .sansMedium, TypeScale.size(16.5))
        title.textColor = vm.unseen || vm.status != .idle ? Palette.text : Palette.text.withAlphaComponent(0.88)
        // The project tile rides the meta line as an attachment, so the text
        // system itself centers it on the name's x-height and starts it at
        // the title's leading edge (a separately framed tile drifted high).
        let metaFont = Fonts.ui(.sans, TypeScale.size(13.5))
        let tileSide = (14 * TypeScale.factor).rounded()
        let art = vm.projectId.flatMap { ProjectArt.tile(spaceId: $0, side: tileSide) }
        let tile = NSTextAttachment(image: art ?? ProjectTile.image(name: vm.hasProject ? vm.projectName : "Home", colorIndex: vm.colorIndex, side: tileSide))
        tile.bounds = CGRect(x: 0, y: (metaFont.xHeight - tileSide) / 2, width: tileSide, height: tileSide)
        let metaText = NSMutableAttributedString(attachment: tile)
        metaText.append(NSAttributedString(string: "  " + vm.projectName, attributes: [.font: metaFont, .foregroundColor: Palette.secondary]))
        if let b = vm.branch, !b.isEmpty, let icon = BranchIcon.image?.withTintColor(Palette.subline, renderingMode: .alwaysOriginal) {
            // Desktop line 3: git-branch icon + branch in the subline tone.
            let font = Fonts.ui(.sans, TypeScale.size(12.5))
            let side = (12 * TypeScale.factor).rounded()
            let attach = NSTextAttachment(image: icon)
            attach.bounds = CGRect(x: 0, y: (font.capHeight - side) / 2, width: side, height: side)
            if metaText.length > 0 { metaText.append(NSAttributedString(string: "   ")) }
            metaText.append(NSAttributedString(attachment: attach))
            metaText.append(NSAttributedString(string: " " + b, attributes: [.font: font, .foregroundColor: Palette.subline]))
        }
        meta.attributedText = metaText
        meta.isHidden = compact
        if !compact, let pr = vm.pr, let n = vm.prNumber {
            prBadge.isHidden = false
            prBadge.configure(number: n, state: pr, size: TypeScale.size(11))
        } else {
            prBadge.isHidden = true
        }
        let corner = Self.corner(vm)
        if let corner {
            time.text = corner.word
            time.textColor = corner.color
            time.font = Fonts.ui(.sansMedium, TypeScale.size(13))
            status.kind = corner.glyph
            status.isHidden = false
        } else {
            time.text = vm.timeLabel
            time.textColor = StatusTone.time
            time.font = Fonts.ui(.sansMedium, TypeScale.size(13))
            status.kind = .none
            status.isHidden = true
        }
        accessibilityLabel = [vm.title, vm.projectName, corner?.word, prBadge.isHidden ? nil : prBadge.accessibilityLabel].compactMap { $0 }.joined(separator: ", ")
        accessibilityIdentifier = "session-\(vm.id)"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let left: CGFloat = 20
        let right: CGFloat = 20
        let k = TypeScale.factor
        let titleH = (22 * k).rounded()
        let titleY = compact ? ((b.height - titleH) / 2).rounded() : (10 * k).rounded()
        let metaY = (35 * k).rounded()
        let metaH = (18 * k).rounded()
        // The mark centers on the title's x-height (font metrics, not the
        // line box): titles are mostly lowercase, and centering on the
        // capitals left the mark riding high — the project tile's rule too.
        let mark = Self.markSide
        let titleFont = title.font ?? Fonts.ui(.sansMedium, TypeScale.size(16.5))
        let baseline = titleY + (titleH - titleFont.lineHeight) / 2 + titleFont.ascender
        harness.frame = CGRect(x: left, y: (baseline - titleFont.xHeight / 2 - mark / 2).rounded(), width: mark, height: mark)
        let textX = left + mark + 14
        let tw = ceil(time.sizeThatFits(CGSize(width: 140, height: 40)).width)
        time.frame = CGRect(x: b.width - right - tw, y: titleY, width: tw, height: titleH)
        var trailing = time.frame.minX - 10
        if !status.isHidden {
            status.frame = CGRect(x: time.frame.minX - 5 - 12, y: titleY + titleH / 2 - 6, width: 12, height: 12)
            trailing = status.frame.minX - 10
        }
        title.frame = CGRect(x: textX, y: titleY, width: max(0, trailing - textX), height: titleH)
        let x = textX
        var metaRight = b.width - right
        if !prBadge.isHidden {
            let size = prBadge.intrinsicContentSize
            prBadge.frame = CGRect(x: b.width - right - size.width, y: metaY + (metaH - size.height) / 2, width: size.width, height: size.height)
            metaRight = prBadge.frame.minX - 10
        }
        meta.frame = CGRect(x: x, y: metaY, width: max(0, metaRight - x), height: metaH)
    }

    /// Harness mark side.
    static let markSide: CGFloat = 20
}

/// Folder row: icon, name, count, chevron.
final class FolderCell: UICollectionViewListCell {
    static var height: CGFloat { (46 * TypeScale.factor).rounded() }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = Self.height
        return attrs
    }

    func configure(_ vm: FolderRowVM) {
        var c = UIListContentConfiguration.cell()
        c.image = UIImage(systemName: vm.symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 16, weight: .regular))
        c.imageProperties.tintColor = Palette.secondary
        c.imageToTextPadding = 18
        let text = NSMutableAttributedString(string: vm.name, attributes: [.font: Fonts.ui(.sansMedium, TypeScale.size(17)), .foregroundColor: Palette.secondary])
        text.append(NSAttributedString(string: "  \(vm.count)", attributes: [.font: Fonts.ui(.sans, TypeScale.size(17)), .foregroundColor: Palette.tertiary]))
        c.attributedText = text
        c.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 20)
        contentConfiguration = c
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
        accessories = [.disclosureIndicator(options: .init(tintColor: Palette.tertiary))]
        accessibilityIdentifier = "folder-\(vm.id)"
    }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : .clear
        bg.cornerRadius = 16
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }
}

/// Foldable section header (desktop sidebar style): name, count, a live
/// glyph when something inside is working, and a disclosure chevron.
final class SectionHeaderCell: UICollectionViewListCell {
    struct State: Equatable {
        let id: String
        var title: String
        var count: Int
        var collapsed: Bool
        var live: StatusGlyph.Kind?
        /// A project group's artwork (or monogram), before the title.
        var icon: UIImage? = nil
    }

    static var height: CGFloat { (40 * TypeScale.factor).rounded() }
    private let title = FadingLabel()
    private let count = UILabel()
    private let chevron = UIImageView(image: UIImage(systemName: "chevron.down", withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .semibold)))
    private let live = StatusGlyph()
    private let icon = UIImageView()
    private var shownCollapsed: Bool?

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.textColor = Palette.secondary
        count.textColor = Palette.tertiary
        chevron.tintColor = Palette.tertiary
        chevron.contentMode = .center
        for v in [icon, title, count, chevron, live] as [UIView] { contentView.addSubview(v) }
        accessibilityTraits = .button
    }

    required init?(coder: NSCoder) { fatalError() }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = Self.height
        return attrs
    }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted ? Palette.controlFill : .clear
        bg.cornerRadius = 12
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 2, leading: 8, bottom: 2, trailing: 8)
        backgroundConfiguration = bg
    }

    func configure(_ s: State) {
        title.font = Fonts.ui(.sansSemibold, TypeScale.size(13.5))
        count.font = Fonts.ui(.sansMedium, TypeScale.size(13.5))
        title.text = s.title
        count.text = "\(s.count)"
        live.isHidden = s.live == nil
        if let kind = s.live { live.kind = kind }
        icon.image = s.icon
        icon.isHidden = s.icon == nil
        let rotate = { self.chevron.transform = s.collapsed ? CGAffineTransform(rotationAngle: -.pi / 2) : .identity }
        if shownCollapsed != nil, shownCollapsed != s.collapsed, window != nil {
            UIView.animate(withDuration: 0.3, delay: 0, usingSpringWithDamping: 0.85, initialSpringVelocity: 0, animations: rotate)
        } else {
            rotate()
        }
        shownCollapsed = s.collapsed
        accessibilityIdentifier = "section-\(s.id)"
        accessibilityLabel = "\(s.title), \(s.count)"
        accessibilityValue = s.collapsed ? "Collapsed" : "Expanded"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let h: CGFloat = 20
        let y = b.height - h - 6
        let tw = ceil(title.sizeThatFits(b.size).width)
        var x: CGFloat = 20
        if !icon.isHidden {
            icon.frame = CGRect(x: x, y: y + h / 2 - 9, width: 18, height: 18)
            x = icon.frame.maxX + 8
        }
        title.frame = CGRect(x: x, y: y, width: min(tw, b.width - x - 100), height: h)
        let cw = ceil(count.sizeThatFits(b.size).width)
        count.frame = CGRect(x: title.frame.maxX + 7, y: y, width: cw, height: h)
        live.frame = CGRect(x: count.frame.maxX + 7, y: y + h / 2 - 6, width: 12, height: 12)
        chevron.frame = CGRect(x: b.width - 20 - 16, y: y, width: 16, height: h)
    }
}
