import O7Marks
import SwiftUI

/// "Lock o7 with Face ID": the app's screens hide behind the wing whenever it leaves the screen,
/// until the person unlocks it with Face ID or the passcode.
struct LockGate: ViewModifier {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase
    @Environment(\.style) private var style
    @State private var locked = true
    @State private var asking = false

    func body(content: Content) -> some View {
        let guarded = model.settings.lockWithFaceId && locked
        content
            .overlay {
                if guarded {
                    ZStack {
                        style.window.ignoresSafeArea()
                        VStack(spacing: 28) {
                            O7Logo()
                                .fill(style.fg1)
                                .aspectRatio(O7Logo.aspectRatio, contentMode: .fit)
                                .frame(width: 120)
                            Button {
                                Task { await unlock() }
                            } label: {
                                Label("Unlock o7", systemImage: "faceid")
                                    .o7Font(.ui(16, weight: .medium))
                                    .padding(.horizontal, 8)
                            }
                            .buttonStyle(.glass)
                            .controlSize(.large)
                        }
                    }
                    .transition(.opacity)
                }
            }
            .animation(.smooth(duration: 0.25), value: guarded)
            .onChange(of: scenePhase) { _, phase in
                switch phase {
                case .background: locked = true
                case .active: if model.settings.lockWithFaceId, locked { Task { await unlock() } }
                default: break
                }
            }
            .task {
                if model.settings.lockWithFaceId { await unlock() } else { locked = false }
            }
    }

    private func unlock() async {
        guard !asking else { return }
        asking = true
        defer { asking = false }
        if await Authenticator.confirm("Unlock o7") { locked = false }
    }
}
