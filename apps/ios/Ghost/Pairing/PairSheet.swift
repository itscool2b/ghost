import GhostClient
import GhostCrypto
import GhostMarks
import SwiftUI

/// Asks before a pairing link is used, then pairs while the person watches each step. Nothing is
/// sent before the tap, as on the web: a program that only opens links cannot spend one.
struct PairSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    let link: PairingLink
    @State private var name = "iPhone"
    /// The pairing this sheet started, kept while the sheet closes so it never flips back.
    @State private var store: ComputerStore?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if let pairing = store {
                Progress(store: pairing, onDone: {
                    model.adopt(pairing)
                    dismiss()
                }, onScanAgain: {
                    model.cancelPairing()
                    dismiss()
                    model.scanning = true
                })
            } else {
                question
            }
        }
        .padding(24)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }

    private var known: ComputerStore? { model.known(link) }

    @ViewBuilder private var question: some View {
        Text(known == nil ? "Pair with this computer?" : "Pair this iPhone again?")
            .ghostFont(.title)
            .foregroundStyle(style.fg1)
            .padding(.bottom, 10)
        Group {
            if let known {
                Text("This iPhone is already paired with \(known.hostName). Pairing again gives it a new key there, and the old one stops working.")
            } else {
                Text("Whoever pairs can read and steer this computer's chats, and its agents can edit files and run commands.")
            }
        }
        .ghostFont(.prose)
        .foregroundStyle(style.fg2)
        Text("Go on only if you opened this code from your own computer just now.")
            .ghostFont(.prose)
            .foregroundStyle(style.fg2)
            .padding(.top, 8)
        VStack(alignment: .leading, spacing: 6) {
            Text("Name on the computer")
                .ghostFont(.meta)
                .foregroundStyle(style.fg3)
            TextField("iPhone", text: $name)
                .ghostFont(.composer)
                .textInputAutocapitalization(.words)
                .submitLabel(.done)
                .padding(.horizontal, 14)
                .frame(minHeight: 46)
                .background(style.control, in: .rect(cornerRadius: 12))
        }
        .padding(.top, 20)
        Spacer(minLength: 24)
        VStack(spacing: 10) {
            ProminentButton(title: known == nil ? "Pair" : "Pair again", systemImage: "link") {
                let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
                model.pair(link, deviceName: trimmed.isEmpty ? "iPhone" : trimmed)
                store = model.pairing
            }
            QuietButton(title: "Not now") {
                model.cancelPairing()
                dismiss()
            }
        }
    }
}

/// The pairing as it happens: each step turns from a loader into a check, then the computer.
private struct Progress: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let store: ComputerStore
    let onDone: () -> Void
    let onScanAgain: () -> Void

    var body: some View {
        if case .ended(let reason, _) = store.phase {
            failed(reason)
        } else if store.isPaired, case .ready = store.phase {
            done
        } else {
            steps
        }
    }

    private var steps: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Pairing")
                .ghostFont(.title)
                .foregroundStyle(style.fg1)
            Step(title: "Reaching the relay", done: store.steps.contains(.relay))
            Step(title: "Proving the code", done: store.steps.contains(.proved))
            Step(title: "Receiving this iPhone's key", done: store.pairedDevice != nil)
            if case .away = store.phase {
                Text("Your computer is not answering. Open Ghost there, and check it is awake and online.")
                    .ghostFont(.meta)
                    .foregroundStyle(style.amber)
            }
            Spacer()
        }
    }

    private var done: some View {
        VStack(alignment: .leading, spacing: 12) {
            AgentMark(agent: "Ghost", state: .done, size: 44, label: "Paired")
            Text("Paired with \(store.hostName)")
                .ghostFont(.title)
                .foregroundStyle(style.fg1)
            Text("Its chats are on this iPhone now. Nothing more to pair: this iPhone opens them without another code.")
                .ghostFont(.prose)
                .foregroundStyle(style.fg2)
            if model.pastedLink {
                // The link is spent, but it named this computer: nothing else needs it now.
                QuietButton(title: "Clear the copied link", systemImage: "doc.on.clipboard") {
                    UIPasteboard.general.items = []
                    model.pastedLink = false
                }
            }
            Spacer()
            if model.push.status == .notDetermined {
                // Asked once, here, when it means something: the computer can now say when an
                // agent waits.
                VStack(alignment: .leading, spacing: 6) {
                    Label("Know when an agent needs you", systemImage: "bell.badge")
                        .ghostFont(.rowTitle)
                        .foregroundStyle(style.fg1)
                    Text("A notification when a chat waits on you, finishes or fails. \(store.hostName) seals what it says for this iPhone alone.")
                        .ghostFont(.meta)
                        .foregroundStyle(style.fg3)
                }
                .padding(.bottom, 8)
                ProminentButton(title: "Turn on notifications", systemImage: "bell") {
                    Task {
                        await model.push.turnOn()
                        onDone()
                    }
                }
                QuietButton(title: "Not now", action: onDone)
            } else {
                ProminentButton(title: "Open \(store.hostName)", systemImage: "arrow.right", action: onDone)
            }
        }
        .sensoryFeedback(.success, trigger: store.isPaired)
    }

    private func failed(_ reason: String) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("This code did not pair")
                .ghostFont(.title)
                .foregroundStyle(style.fg1)
            Text(reason)
                .ghostFont(.prose)
                .foregroundStyle(style.fg2)
            Spacer()
            ProminentButton(title: "Scan a new code", systemImage: "qrcode.viewfinder", action: onScanAgain)
        }
        .sensoryFeedback(.error, trigger: reason)
    }
}

private struct Step: View {
    @Environment(\.style) private var style
    let title: String
    let done: Bool

    var body: some View {
        HStack(spacing: 12) {
            ZStack {
                if done {
                    Image(systemName: "checkmark")
                        .font(.footnote.weight(.bold))
                        .foregroundStyle(style.green)
                        .transition(.scale.combined(with: .opacity))
                } else {
                    PixelLoader(size: 16, tempo: .thinking)
                }
            }
            .frame(width: 22, height: 22)
            Text(title)
                .ghostFont(.ui(16, weight: done ? .regular : .medium))
                .foregroundStyle(done ? style.fg2 : style.fg1)
        }
        .animation(.snappy, value: done)
        .accessibilityElement(children: .combine)
        .accessibilityValue(done ? "Done" : "In progress")
    }
}
