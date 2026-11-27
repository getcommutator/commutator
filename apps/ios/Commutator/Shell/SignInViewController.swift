import AuthenticationServices
import UIKit

/// Signed out: the welcome page — the mark, a line, and "Continue with
/// Google" (the hosted server's sign-in, in the system web-auth sheet, coming
/// back to `commutator://auth`).
final class SignInViewController: UIViewController {
    /// The hosted Commutator server every app signs in to.
    static let serverURL = "https://commutator.freethinkel.dev"

    private let app: AppModel
    private let status = UILabel()
    private var start: UIButton!
    private let signIn = ServerWebSignIn()

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        let wallpaper = WallpaperView()
        wallpaper.frame = view.bounds
        wallpaper.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(wallpaper)
        // The icon's drawing on its own: a template mask, so it takes the text colour in either theme.
        let mark = UIImageView(image: UIImage(named: "welcome-mark")?.withRenderingMode(.alwaysTemplate))
        mark.tintColor = Palette.text
        mark.contentMode = .scaleAspectFit
        mark.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            mark.widthAnchor.constraint(equalToConstant: 120),
            mark.heightAnchor.constraint(equalToConstant: 120),
        ])
        let title = UILabel()
        title.text = "Welcome to Commutator"
        title.font = Fonts.ui(.sansSemibold, 30)
        title.textColor = Palette.text
        title.textAlignment = .center
        title.numberOfLines = 0
        let tagline = UILabel()
        tagline.text = "Your coding agents, from anywhere."
        tagline.font = Fonts.ui(.sans, 17)
        tagline.textColor = Palette.secondary
        tagline.textAlignment = .center
        tagline.numberOfLines = 0

        status.font = Fonts.ui(.sans, 14)
        status.textColor = Palette.secondary
        status.textAlignment = .center
        status.numberOfLines = 0

        start = Controls.primaryButton("Continue with Google") { [weak self] in self?.go() }
        start.accessibilityIdentifier = "sign-in"

        let top = UIStackView(arrangedSubviews: [mark, title, tagline])
        top.axis = .vertical
        top.alignment = .center
        top.spacing = 12
        top.setCustomSpacing(26, after: mark)
        let demo = Controls.plainButton("Try the demo") { [weak self] in self?.app.startDemo() }
        demo.accessibilityIdentifier = "try-demo"

        let bottom = UIStackView(arrangedSubviews: [status, start, demo])
        bottom.axis = .vertical
        bottom.spacing = 16
        bottom.setCustomSpacing(4, after: start)
        for v in [top, bottom] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            top.centerXAnchor.constraint(equalTo: guide.centerXAnchor),
            top.centerYAnchor.constraint(equalTo: guide.centerYAnchor, constant: -80),
            top.leadingAnchor.constraint(greaterThanOrEqualTo: guide.leadingAnchor, constant: 32),
            bottom.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 24),
            bottom.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -24),
            bottom.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -16),
        ])
    }

    private func go() {
        start.isEnabled = false
        status.text = nil
        Task { @MainActor in
            let error = await signIn.run(app: app, serverURL: Self.serverURL, from: self)
            start.isEnabled = true
            status.text = error
        }
    }
}

/// The server's Google sign-in in the system web-auth sheet: the core builds
/// the PKCE URL, the sheet comes back to `commutator://auth?code=…`, the core
/// redeems the code for this phone's token.
@MainActor
final class ServerWebSignIn: NSObject, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?
    private weak var host: UIViewController?

    /// Signs in; returns why it didn't (nil on success or cancel).
    func run(app: AppModel, serverURL: String, from host: UIViewController) async -> String? {
        self.host = host
        let signIn: ServerSignIn
        do {
            signIn = try ServerSignIn(serverUrl: serverURL, deviceName: UIDevice.current.name)
        } catch {
            return "That server address doesn't look right."
        }
        guard let start = URL(string: signIn.url()) else { return "That server address doesn't look right." }
        let url: URL? = await withCheckedContinuation { cont in
            let s = ASWebAuthenticationSession(url: start, callback: .customScheme(authCallbackScheme())) { url, error in
                if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
                    return cont.resume(returning: nil)
                }
                cont.resume(returning: url ?? URL(string: "commutator://auth?error=failed"))
            }
            s.presentationContextProvider = self
            s.prefersEphemeralWebBrowserSession = false
            session = s
            if !s.start() { cont.resume(returning: nil) }
        }
        session = nil
        guard let url else { return nil }
        do {
            let account = try await signIn.finish(callbackUrl: url.absoluteString)
            app.signedIn(account)
            return nil
        } catch {
            NSLog("sign-in failed: \(error)")
            return "Sign-in didn't complete. Try again."
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        host?.view.window ?? ASPresentationAnchor()
    }
}
