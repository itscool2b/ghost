import O7Crypto
import O7Marks
import SwiftUI

@main
struct O7App: App {
    @State private var model: AppModel
    @Environment(\.scenePhase) private var scenePhase

    init() {
        _ = O7Fonts.register()
        _model = State(initialValue: AppModel())
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .onAppear { model.start() }
                // A pairing link opened as a link: the o7.codes address (a Universal Link), or in a
                // development build the o7:// form a simulator can open.
                .onOpenURL { url in model.open(url) }
                .onContinueUserActivity(NSUserActivityTypeBrowsingWeb) { activity in
                    if let url = activity.webpageURL { model.open(url) }
                }
        }
        .onChange(of: scenePhase) { _, phase in
            // Back in front: every session checks itself at once instead of waiting out its ping.
            if phase == .active { model.wakeAll() }
        }
    }
}

extension AppModel {
    /// A link the system handed the app.
    func open(_ url: URL) {
        if url.scheme == "o7" {
            #if O7_DEV_TOOLS
                // o7://pair?link=<a pairing link>, for a simulator, which cannot open Universal Links
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
