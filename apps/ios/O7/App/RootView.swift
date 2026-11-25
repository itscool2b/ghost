import O7Theme
import O7Wire
import SwiftUI

/// The app's one screen tree: the welcome until a computer is paired, then the tabs; the pairing
/// sheet over either whenever a link arrives.
struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.colorScheme) private var systemScheme

    var body: some View {
        @Bindable var model = model
        let style = O7Style.of(model.look, systemDark: systemScheme == .dark)
        Group {
            if model.computers.isEmpty {
                WelcomeView()
            } else {
                RootTabs()
            }
        }
        .modifier(LockGate())
        .o7Look(style)
        .tint(style.accent)
        .preferredColorScheme(model.settings.appearance == nil && model.look.appearance == "system" ? nil : style.scheme)
        .animation(.smooth(duration: 0.35), value: style)
        .fullScreenCover(isPresented: $model.scanning) {
            ScannerView().o7Look(style)
        }
        .sheet(item: $model.proposed, onDismiss: { model.cancelPairing() }) { link in
            PairSheet(link: link)
                .o7Look(style)
                .presentationDetents([.medium, .large])
                .presentationBackground(style.raised)
                .interactiveDismissDisabled(model.pairing != nil && model.pairing?.isPaired == false)
        }
        .alert(
            "Not an o7 code",
            isPresented: Binding(get: { model.notice != nil }, set: { if !$0 { model.notice = nil } })
        ) {
            Button("OK", role: .cancel) { model.notice = nil }
        } message: {
            Text(model.notice ?? "")
        }
    }
}
