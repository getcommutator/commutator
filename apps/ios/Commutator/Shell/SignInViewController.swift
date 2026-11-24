import AuthenticationServices
import UIKit

/// Signed out: the welcome page — the mark, a line, the account's Commutator
/// server, and "Continue with Google" (the server's sign-in, in the system
/// web-auth sheet, coming back to `commutator://auth`).
final class SignInViewController: UIViewController, UITextFieldDelegate {
    private let app: AppModel
    private let server = UITextField()
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
        let mark = UIImageView(image: Controls.commutatorMark)
        mark.tintColor = Palette.text
        mark.contentMode = .scaleAspectFit
        mark.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            mark.widthAnchor.constraint(equalToConstant: 53),
            mark.heightAnchor.constraint(equalToConstant: 53),
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

        server.text = AppModel.lastServerURL
        server.placeholder = "https://commutator.example.com"
        server.font = Fonts.ui(.sans, 16)
        server.textColor = Palette.text
        server.keyboardType = .URL
        server.autocapitalizationType = .none
        server.autocorrectionType = .no
        server.returnKeyType = .go
        server.clearButtonMode = .whileEditing
        server.delegate = self
        server.accessibilityIdentifier = "server-url"
        server.borderStyle = .roundedRect
        server.backgroundColor = Palette.background.withAlphaComponent(0.6)
        let serverLabel = UILabel()
        serverLabel.text = "Your Commutator server"
        serverLabel.font = Fonts.ui(.sansMedium, 13)
        serverLabel.textColor = Palette.secondary

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
        let bottom = UIStackView(arrangedSubviews: [serverLabel, server, status, start])
        bottom.axis = .vertical
        bottom.spacing = 8
        bottom.setCustomSpacing(16, after: server)
        bottom.setCustomSpacing(16, after: status)
        for v in [top, bottom] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            top.centerXAnchor.constraint(equalTo: guide.centerXAnchor),
            top.centerYAnchor.constraint(equalTo: guide.centerYAnchor, constant: -80),
            top.leadingAnchor.constraint(greaterThanOrEqualTo: guide.leadingAnchor, constant: 32),
            server.heightAnchor.constraint(equalToConstant: 44),
            bottom.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 24),
            bottom.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -24),
            bottom.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -16),
        ])
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        go()
        return true
    }

    private func go() {
        let url = (server.text ?? "").trimmingCharacters(in: .whitespaces)
        guard url.hasPrefix("https://") || url.hasPrefix("http://") else {
            status.text = "Enter your server's address, starting with https://"
            return
        }
        view.endEditing(true)
        start.isEnabled = false
        status.text = nil
        Task { @MainActor in
            let error = await signIn.run(app: app, serverURL: url, from: self)
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
