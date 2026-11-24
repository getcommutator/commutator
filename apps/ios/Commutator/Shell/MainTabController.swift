import UIKit

/// Root on iPhone: tabs — Chats, Pull requests, Settings. New session and
/// search sit in the Chats tab's navigation bar. Full-screen pages (a
/// session, a folder) push on an outer stack over the whole tab controller,
/// so the tab bar stays part of the page underneath and slides with it,
/// never fading in over the page that leaves.
final class MainTabController: UIViewController, AppRouter, UINavigationControllerDelegate {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    private lazy var sessionsList = SessionsViewController(app: app)
    private lazy var nav = sessionsNav()
    private lazy var tabs = makeTabs()
    /// The outer stack: the tabs at its root, full-screen pages above.
    private lazy var stack = makeStack()

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        addChild(stack)
        stack.view.frame = view.bounds
        stack.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(stack.view)
        stack.didMove(toParent: self)
    }

    override var childForStatusBarStyle: UIViewController? { stack }

    private func makeStack() -> UINavigationController {
        let stack = Self.nav(tabs)
        stack.setNavigationBarHidden(true, animated: false)
        stack.delegate = self
        // The tabs have no title of their own to go back to.
        tabs.navigationItem.backButtonDisplayMode = .minimal
        return stack
    }

    /// The outer bar is the pushed page's; the tabs bring their own.
    func navigationController(_ navigationController: UINavigationController, willShow viewController: UIViewController, animated: Bool) {
        navigationController.setNavigationBarHidden(viewController === tabs, animated: animated)
    }

    private func makeTabs() -> UITabBarController {
        let nav = self.nav
        let prs = Self.nav(PullRequestsViewController(), tab: true)
        let settings = Self.nav(MoreViewController(app: app), tab: true)
        let tabs = UITabBarController(tabs: [
            UITab(title: "Chats", image: UIImage(systemName: "bubble.left.and.bubble.right"), identifier: "chats") { _ in nav },
            UITab(title: "Pull requests", image: UIImage(systemName: "arrow.triangle.pull"), identifier: "pulls") { _ in prs },
            UITab(title: "Settings", image: UIImage(systemName: "gearshape"), identifier: "settings") { _ in settings },
        ])
        tabs.tabBar.tintColor = Palette.text
        return tabs
    }

    private func sessionsNav() -> UINavigationController {
        sessionsList.enableToolbarSearch()
        sessionsList.loadViewIfNeeded()
        sessionsList.navigationItem.rightBarButtonItems = [sessionsList.navigationItem.rightBarButtonItem, composeItem].compactMap { $0 }
        return Self.nav(sessionsList, tab: true)
    }

    private lazy var composeItem: UIBarButtonItem = {
        let item = UIBarButtonItem(image: UIImage(systemName: "square.and.pencil"), primaryAction: UIAction { [weak self] _ in self?.presentNewSession() })
        item.accessibilityLabel = "New session"
        item.accessibilityIdentifier = "new-session"
        return item
    }()

    /// The tab bar's frame when it's on screen (toast placement).
    func bottomBarFrame(in window: UIWindow) -> CGRect? {
        let bar = tabs.tabBar
        guard stack.topViewController === tabs, !tabs.isTabBarHidden, !bar.isHidden, bar.window != nil else { return nil }
        return bar.convert(bar.bounds, to: window)
    }

    static func nav(_ root: UIViewController, tab: Bool = false) -> UINavigationController {
        let nav = tab ? TabNavigationController(rootViewController: root) : UINavigationController(rootViewController: root)
        nav.navigationBar.prefersLargeTitles = true
        // Setting an appearance object drops iOS 26's scroll-edge effect;
        // style titles through the bar's own attributes instead.
        nav.navigationBar.largeTitleTextAttributes = [.font: Fonts.ui(.sansSemibold, 30), .foregroundColor: Palette.text]
        nav.navigationBar.titleTextAttributes = [.font: Fonts.ui(.sansSemibold, 17), .foregroundColor: Palette.text]
        nav.navigationBar.tintColor = Palette.text
        return nav
    }

    func presentNewSession(prompt: String? = nil) {
        let vc = NewSessionViewController(app: app, prompt: prompt) { [weak self] chatId, handoff in
            guard let self else { return }
            self.openSession(chatId, handoff: handoff)
        }
        let nav = UINavigationController(rootViewController: vc)
        nav.modalPresentationStyle = .pageSheet
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
        present(nav, animated: true)
    }

    func showSettings() {
        if presentedViewController != nil { dismiss(animated: false) }
        stack.popToRootViewController(animated: false)
        tabs.selectedTab = tabs.tabs.first { $0.identifier == "settings" }
    }

    func showSearch() {
        popToFrontPage()
        sessionsList.activateSearch()
    }

    /// From the new-session sheet: the chat goes in under the sheet at once,
    /// the sheet leaves without its slide, and the handoff animation carries
    /// the draft (page, composer, message) into the chat.
    func openSession(_ chatId: String, handoff: DraftHandoff?) {
        guard let handoff, let window = view.window else { return openSession(chatId) }
        guard let session = SessionViewController(app: app, chatId: chatId) else {
            return showSessionOpenError()
        }
        UIView.performWithoutAnimation {
            showChats()
            popToChats()
            stack.pushViewController(session, animated: false)
            view.layoutIfNeeded()
            session.prepareArrival()
            // Sending puts the keyboard away; it slides down with the handoff.
            presentedViewController?.view.endEditing(true)
            // Hide the sheet now; tear it down only once the motion is done.
            // Dismissal (plus the keyboard re-hosting with it) is a heavy
            // compositor frame — mid-animation it swallowed the motion.
            if let sheet = presentedViewController?.presentationController as? UISheetPresentationController {
                // Drop the sheet's background dimming with it (it would sit over
                // the chat until the deferred dismissal, then snap off).
                sheet.largestUndimmedDetentIdentifier = .large
            }
            presentedViewController?.presentationController?.containerView?.alpha = 0
        }
        DraftHandoffAnimator.run(handoff, into: session, window: window) { [weak self] in
            if self?.presentedViewController != nil { self?.dismiss(animated: false) }
        }
    }

    /// Back to the Sessions list, releasing any pushed session (the iPad
    /// split takes the open session back into its own column).
    func popToFrontPage() {
        showChats()
        popToChats()
    }

    /// The split's open session, handed over as it is when the window
    /// narrows (iPhone Duo folding): same page, scroll position and draft.
    func adoptSession(_ session: SessionViewController) {
        showChats()
        popToChats()
        stack.pushViewController(session, animated: false)
    }

    /// The pushed session, taken off the stack for the split's main column
    /// when the window widens; nil when none is open. It comes back marked
    /// `relocating`, for the split to clear once it's placed.
    func releaseSession() -> SessionViewController? {
        // The session itself, not a side chat pushed above it.
        guard let session = stack.viewControllers.first(where: { ($0 as? SessionViewController)?.isCompanion == false }) as? SessionViewController else {
            return nil
        }
        session.relocating = true
        popToChats()
        return session
    }

    func showSessionOpenError() {
        if presentedViewController != nil { dismiss(animated: false) }
        view.endEditing(true)
        popToFrontPage()
        view.layoutIfNeeded()
        Toast.show("Couldn't open session. Try again.", in: view.window)
    }

    /// Push a session on the Sessions list (from new-session, deep links, search).
    func openSession(_ chatId: String) {
        guard let session = SessionViewController(app: app, chatId: chatId) else {
            return showSessionOpenError()
        }
        if presentedViewController != nil { dismiss(animated: true) }
        showChats()
        popToChats()
        stack.pushViewController(session, animated: true)
    }

    private func showChats() {
        tabs.selectedTab = tabs.tabs.first { $0.identifier == "chats" }
    }

    /// Off every pushed page, back to the Chats list.
    private func popToChats() {
        stack.popToRootViewController(animated: false)
        nav.popToRootViewController(animated: false)
    }
}

/// A tab's own stack. A page that hides the tab bar goes on the shell's
/// outer stack instead, over the tab bar rather than under it.
final class TabNavigationController: UINavigationController {
    override func pushViewController(_ viewController: UIViewController, animated: Bool) {
        if viewController.hidesBottomBarWhenPushed, let outer = navigationController {
            return outer.pushViewController(viewController, animated: animated)
        }
        super.pushViewController(viewController, animated: animated)
    }
}

/// The Pull requests tab. ponytail: empty until the server serves PRs.
final class PullRequestsViewController: UIViewController {
    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Pull requests"
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .always
        var empty = UIContentUnavailableConfiguration.empty()
        empty.image = UIImage(systemName: "arrow.triangle.pull")
        empty.text = "No pull requests"
        empty.secondaryText = "Pull requests from your sessions will show up here."
        contentUnavailableConfiguration = empty
    }
}

extension UIViewController {
    /// The iPhone shell this screen sits in, if any (not beside the iPad
    /// sidebar, not a standalone sheet).
    var mainShell: MainTabController? {
        var ancestor = parent
        while let current = ancestor {
            if let shell = current as? MainTabController { return shell }
            ancestor = current.parent
        }
        return nil
    }
}
