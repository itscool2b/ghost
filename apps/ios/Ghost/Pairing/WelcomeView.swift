import GhostMarks
import SwiftUI

/// The first screen, before any computer is paired: what the app is for, and the two ways to pair.
struct WelcomeView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var arrived = false

    var body: some View {
        VStack(spacing: 0) {
            Spacer()
            // The wing flies in once: this is setup, the one place it moves.
            GhostLogo()
                .fill(style.fg1)
                .aspectRatio(1044 / 440, contentMode: .fit)
                .frame(width: 148)
                .offset(x: arrived || reduceMotion ? 0 : -60, y: arrived || reduceMotion ? 0 : 24)
                .opacity(arrived ? 1 : 0)
                .accessibilityLabel("Ghost")
                .padding(.bottom, 36)
            Text("Ghost on your iPhone")
                .ghostFont(.display)
                .foregroundStyle(style.fg1)
                .multilineTextAlignment(.center)
            Text("Follow and steer the agents on your computer.")
                .ghostFont(.prose)
                .foregroundStyle(style.fg2)
                .multilineTextAlignment(.center)
                .padding(.top, 8)
            Spacer()
            VStack(spacing: 12) {
                ProminentButton(title: "Scan pairing code", systemImage: "qrcode.viewfinder") {
                    model.scanning = true
                }
                HStack(spacing: 10) {
                    Text("Copied the link instead?")
                        .ghostFont(.meta)
                        .foregroundStyle(style.fg3)
                    PasteLinkButton()
                }
                .padding(.vertical, 4)
                if model.backend.mock != nil {
                    QuietButton(title: "Use the demo computer", systemImage: "desktopcomputer") {
                        Task { await model.pairDemo() }
                    }
                }
            }
            Text("On your computer: Ghost Settings, Remote access, Pair a device.")
                .ghostFont(.meta)
                .foregroundStyle(style.fg3)
                .multilineTextAlignment(.center)
                .padding(.top, 18)
        }
        .padding(.horizontal, 24)
        .padding(.bottom, 20)
        .frame(maxWidth: 520)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(style.window.ignoresSafeArea())
        .onAppear {
            withAnimation(.spring(response: 0.9, dampingFraction: 0.82).delay(0.1)) { arrived = true }
        }
    }
}

/// Pastes a pairing link from the clipboard without the system asking first: the person taps the
/// system's own paste control.
struct PasteLinkButton: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style

    var body: some View {
        PasteButton(payloadType: String.self) { strings in
            Task { @MainActor in model.propose(strings.first ?? "", pasted: true) }
        }
        .labelStyle(.titleAndIcon)
        .buttonBorderShape(.capsule)
        .tint(style.primary)
        .controlSize(.regular)
        .accessibilityHint("Pastes a pairing link you copied from Ghost on your computer")
    }
}
