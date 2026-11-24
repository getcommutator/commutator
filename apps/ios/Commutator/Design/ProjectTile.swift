import UIKit

/// The desktop's project monogram: a rounded tile in the project's tone
/// (fill @ 0.08, letter @ 0.85, mono medium) — `ui/src/shell/project_icon.rs`.
enum ProjectTile {
    static func letter(_ name: String) -> String {
        String(name.trimmingCharacters(in: .whitespaces).first ?? "?").uppercased()
    }

    /// The tile as an image (composer chips), light/dark aware.
    static func image(name: String, colorIndex: Int, side: CGFloat = 14) -> UIImage {
        let tone = Palette.projectDots[colorIndex % Palette.projectDots.count]
        let asset = UIImageAsset()
        let scale = UITraitCollection.current.displayScale > 0 ? UITraitCollection.current.displayScale : 3
        let format = UIGraphicsImageRendererFormat()
        format.scale = scale
        for style in [UIUserInterfaceStyle.light, .dark] {
            let traits = UITraitCollection { $0.userInterfaceStyle = style; $0.displayScale = scale }
            let t = tone.resolvedColor(with: traits)
            let img = UIGraphicsImageRenderer(size: CGSize(width: side, height: side), format: format).image { _ in
                let k = side / 13
                t.withAlphaComponent(0.08).setFill()
                UIBezierPath(roundedRect: CGRect(x: 0, y: 0, width: side, height: side), cornerRadius: 3 * k).fill()
                let font = Fonts.ui(.monoMedium, 9 * k)
                let s = NSAttributedString(string: letter(name), attributes: [.font: font, .foregroundColor: t.withAlphaComponent(0.85)])
                let size = s.size()
                s.draw(at: CGPoint(x: (side - size.width) / 2, y: side / 2 - (font.ascender - font.capHeight / 2)))
            }.withRenderingMode(.alwaysOriginal)
            asset.register(img, with: traits)
        }
        return asset.image(with: UITraitCollection { $0.userInterfaceStyle = UITraitCollection.current.userInterfaceStyle; $0.displayScale = scale })
    }
}

/// The desktop's repository artwork (`project_icon.rs` ICON_PATHS), read
/// over the attachment route, which serves files under any chat's folder.
/// A project with no chats yet, or on a device out of reach, keeps its
/// monogram until the next try. Main thread only.
enum ProjectArt {
    /// Posted when artwork lands: lists redraw their rows.
    static let loaded = Notification.Name("ProjectArtLoaded")
    // ponytail: no SVG (UIImage can't decode it); add a renderer when a project needs one.
    private static let paths = [
        "public/apple-touch-icon.png", "apple-touch-icon.png", "public/favicon.png",
        "public/icon.png", "public/logo.png", "favicon.png", "app/icon.png", "src/app/icon.png",
        "public/favicon.ico", "favicon.ico", "app/favicon.ico", "static/favicon.ico",
        "src-tauri/icons/icon.png", "assets/icon.png", "src/assets/icon.png",
    ]
    private static var art: [String: UIImage] = [:]
    /// Last try per project that found nothing: retried after five minutes, as the desktop refreshes.
    private static var tried: [String: Date] = [:]
    /// Each checkout's project id → its repository's artwork key.
    private static var keyBySpace: [String: String] = [:]
    private static var tiles: [String: UIImage] = [:]

    static func link(spaceId: String, device: String, root: String) {
        keyBySpace[spaceId] = device + ":" + root
    }

    /// A project's artwork clipped to the monogram tile's rounding, at
    /// `side`; nil until loaded.
    static func tile(spaceId: String, side: CGFloat) -> UIImage? {
        guard let key = keyBySpace[spaceId], let img = art[key] else { return nil }
        let id = "\(key)@\(side)"
        if let hit = tiles[id] { return hit }
        let rect = CGRect(x: 0, y: 0, width: side, height: side)
        let tile = UIGraphicsImageRenderer(size: rect.size).image { _ in
            UIBezierPath(roundedRect: rect, cornerRadius: side * 0.23).addClip()
            img.draw(in: rect)
        }
        tiles[id] = tile
        return tile
    }

    /// The artwork if loaded; otherwise starts loading it and calls `loaded` when it lands.
    static func image(client: CoreClient?, device: String, root: String, loaded: @escaping () -> Void) -> UIImage? {
        let key = device + ":" + root
        if let hit = art[key] { return hit }
        guard let client, !root.isEmpty, Date().timeIntervalSince(tried[key] ?? .distantPast) > 300 else { return nil }
        tried[key] = Date()
        Task { @MainActor in
            for p in paths {
                guard let data = try? await client.readAttachment(deviceId: device, path: (root as NSString).appendingPathComponent(p)) else { continue }
                // As the desktop: the first file found decides, even a corrupt one.
                guard let img = UIImage(data: data) else { return }
                art[key] = img
                tried[key] = nil
                NotificationCenter.default.post(name: Self.loaded, object: nil)
                return loaded()
            }
        }
        return nil
    }
}

/// The desktop's `git-branch.svg` (template).
enum BranchIcon {
    static let image: UIImage? = UIImage(named: "tool-git-branch")

    /// Rasterized at `side` points (buttons size images intrinsically).
    static func sized(_ side: CGFloat = 13) -> UIImage? {
        guard let image else { return nil }
        return UIGraphicsImageRenderer(size: CGSize(width: side, height: side)).image { _ in
            image.draw(in: CGRect(x: 0, y: 0, width: side, height: side))
        }.withRenderingMode(.alwaysTemplate)
    }
}
