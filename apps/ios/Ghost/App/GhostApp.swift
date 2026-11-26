import GhostCrypto
import GhostMarks
import SwiftUI

@main
struct GhostApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase

    init() {
        _ = GhostFonts.register()
        _model = State(initialValue: AppModel())
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onAppear { model.start() }
                // A pairing link opened as a link: the onghost.dev address (a Universal Link), or in a
                // development build the ghost:// form a simulator can open.
                .onOpenURL { url in model.open(url) }
                .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                    if let url = activity.webpageURL { model.open(url) }
                }
        }
        .onChange(of: scenePhase) { _, phase in
            // Back in front: every session checks itself at once instead of waiting out its ping.
            if phase == .active {
                model.wakeAll()
                Task { await model.push.refreshStatus() }
            }
            // Leaving: the widgets, the badge and the Live Activity take what the app last knew.
            if phase == .background { model.writeSnapshot() }
        }
    }
}

extension AppModel {
    /// A link the system handed the app.
    func open(_ url: URL) {
        // ghost://inbox and ghost://chat/<computer>/<chat>, from the widgets and the Live Activity.
        if url.scheme == "ghost", url.host() == "inbox" {
            route = nil
            return
        }
        if url.scheme == "ghost", url.host() == "chat" {
            let parts = url.pathComponents.filter { $0 != "/" }
            if parts.count == 2, computers.contains(where: { $0.hostId == parts[0] }) {
                select(parts[0])
                route = ChatRoute(hostId: parts[0], chatId: parts[1])
            }
            return
        }
        if url.scheme == "ghost" {
            #if GHOST_DEV_TOOLS
                // ghost://pair?link=<a pairing link>, for a simulator, which cannot open Universal Links
                // without a team.
                if let link = URLComponents(url: url, resolvingAgainstBaseURL: false)?
                    .queryItems?.first(where: { $0.name == "link" })?.value
                {
                    propose(link)
                }
            #endif
            return
        }
        propose(url.absoluteString)
    }
}

extension PairingLink: @retroactive Identifiable {
    public var id: String { pairingId }
}
