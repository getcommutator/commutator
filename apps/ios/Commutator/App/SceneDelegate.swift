import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let app = AppModel()

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let scene = scene as? UIWindowScene else { return }
        // The selected themes, before anything paints (the core's visuals too).
        ThemeStore.load()
        let window = UIWindow(windowScene: scene)
        window.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: UserDefaults.standard.integer(forKey: "appearance")) ?? .unspecified
        window.tintColor = Palette.accent
        self.window = window
        let push = PushNotifications.shared
        push.install(app: app)
        push.openChat = { [weak self] chatId in
            guard let router = self?.window?.rootViewController as? AppRouter else { return false }
            router.openSession(chatId)
            return true
        }
        app.onSignedIn = { [weak self] in
            self?.showRoot(animated: true)
            self?.shellShown()
        }
        app.onSignOut = { [weak self] in
            guard let self else { return }
            push.signingOut()
            self.app.signOutLocally()
            self.showRoot(animated: true)
        }
        let args = ProcessInfo.processInfo.arguments
        // `-wallpaper <path> [-wallpaper-effect ascii]`: set the wallpaper at
        // launch (screenshots, tests); `-wallpaper none` clears it.
        if let i = args.firstIndex(of: "-wallpaper"), i + 1 < args.count {
            if args[i + 1] == "none" {
                WallpaperStore.remove()
            } else if let image = UIImage(contentsOfFile: args[i + 1]) {
                WallpaperStore.set(image, name: (args[i + 1] as NSString).lastPathComponent)
            }
        }
        if let i = args.firstIndex(of: "-wallpaper-effect"), i + 1 < args.count,
           let effect = WallpaperStore.allEffects.first(where: { WallpaperStore.key($0) == args[i + 1] }) {
            WallpaperStore.effect = effect
        }
        if args.contains("-lab") {
            window.rootViewController = MainTabController.nav(TranscriptLabViewController())
        } else {
            showRoot(animated: false)
        }
        window.makeKeyAndVisible()
        if !args.contains("-lab") { shellShown() }

        #if DEBUG
        TerminalProbe.runIfAsked(app: app, router: window.rootViewController as? AppRouter)
        #endif
        if let router = window.rootViewController as? AppRouter, let i = args.firstIndex(of: "-route"), i + 1 < args.count {
            let route = args[i + 1]
            DispatchQueue.main.async {
                if route.hasPrefix("chat:") { router.openSession(String(route.dropFirst(5))) }
                if route == "new" { router.presentNewSession(prompt: nil) }
                if route == "more" { router.showSettings() }
                if route == "search" { router.showSearch() }
            }
        }
    }

    /// The signed-in shell is up: refresh this phone's notification
    /// registration and open a session a notification was tapped for.
    private func shellShown() {
        guard app.account != nil else { return }
        if !app.isDemo { PushNotifications.shared.signedIn() }
        guard app.isSignedIn else { return }
        // After the root swap, so the router is the new shell.
        DispatchQueue.main.async { PushNotifications.shared.shellReady() }
    }

    private func showRoot(animated: Bool) {
        guard let window else { return }
        // The split shell everywhere: it shows the sidebar when the window
        // is regular in both directions (iPad, an unfolded iPhone Duo) and
        // collapses to the tab shell otherwise, live as the window changes —
        // folding or unfolding carries the open session across.
        let root: UIViewController = app.isSignedIn ? SplitRootController(app: app)
            : app.account != nil ? UINavigationController(rootViewController: MachinePickerViewController(app: app))
            : SignInViewController(app: app)
        // Sheets (new session, Settings) belong to the old root: they'd
        // stay on top of the new one.
        if let old = window.rootViewController, old.presentedViewController != nil {
            old.dismiss(animated: false)
        }
        guard animated, window.rootViewController != nil else {
            window.rootViewController = root
            return
        }
        UIView.transition(with: window, duration: 0.35, options: [.transitionCrossDissolve, .allowAnimatedContent]) {
            window.rootViewController = root
        }
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        app.didEnterBackground()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        app.willEnterForeground()
    }
}
