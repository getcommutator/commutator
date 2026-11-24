import UIKit

/// Signed in, no machine picked yet: the account's machines (from its
/// Commutator server), online first. Picking one drives its engine.
final class MachinePickerViewController: UITableViewController {
    private let app: AppModel
    private var machines: [ServerDevice] = []
    private var failure: String?
    private var loading = true

    init(app: AppModel) {
        self.app = app
        super.init(style: .insetGrouped)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Machines"
        navigationController?.navigationBar.prefersLargeTitles = true
        tableView.backgroundColor = Palette.background
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "machine")
        tableView.accessibilityIdentifier = "machines"
        refreshControl = UIRefreshControl()
        refreshControl?.addAction(UIAction { [weak self] _ in self?.load() }, for: .valueChanged)
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Sign Out", primaryAction: UIAction { [weak self] _ in self?.app.signOut() })
        load()
    }

    private func load() {
        Task { @MainActor in
            do {
                let all = try await app.machines()
                machines = all.filter(\.isMachine).sorted { ($0.online ? 0 : 1, $0.name) < ($1.online ? 0 : 1, $1.name) }
                failure = nil
            } catch CoreError.Auth {
                // The server no longer knows this phone (removed there).
                app.signOut()
                return
            } catch {
                failure = "Couldn't reach your server. Pull to try again."
            }
            loading = false
            refreshControl?.endRefreshing()
            tableView.reloadData()
        }
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int {
        machines.count
    }

    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? {
        "Pick the computer to work on"
    }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        if let failure { return failure }
        if loading { return "Loading…" }
        if machines.isEmpty { return "No machines yet. Run `commutator login` on a computer to add it to \(app.accountDetail)." }
        return "Signed in as \(app.accountName)."
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let machine = machines[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "machine", for: indexPath)
        var c = UIListContentConfiguration.subtitleCell()
        c.text = machine.name
        c.textProperties.font = Fonts.ui(.sansMedium, 17)
        c.secondaryText = machine.online ? "Online" : "Offline · last seen \(relativeTimeLabel(atMs: machine.lastSeen * 1000, nowMs: Int64(Date().timeIntervalSince1970 * 1000)))"
        c.secondaryTextProperties.color = machine.online ? .systemGreen : Palette.secondary
        c.image = UIImage(systemName: "desktopcomputer")
        c.imageProperties.tintColor = machine.online ? Palette.text : Palette.secondary
        cell.contentConfiguration = c
        cell.accessoryType = machine.id == app.host?.id ? .checkmark : .disclosureIndicator
        cell.accessibilityIdentifier = "machine-\(machine.name)"
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        app.choose(machines[indexPath.row])
    }
}
