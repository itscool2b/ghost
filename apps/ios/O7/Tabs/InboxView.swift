import LocalAuthentication
import O7Client
import O7Marks
import O7Wire
import SwiftUI

/// What matters now on the computer in view: what needs the person, what works, what just ended.
struct InboxView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let zoom: Namespace.ID
    @Binding var settings: Bool
    @Binding var newWork: Bool
    let open: (ChatRoute) -> Void

    var body: some View {
        Group {
            if let computer = model.selected {
                content(computer)
            }
        }
        .background(style.window.ignoresSafeArea())
        .toolbar { TabToolbar(settings: $settings, newWork: $newWork) }
        .toolbarTitleMenu { ComputerMenu() }
    }

    @ViewBuilder private func content(_ computer: ComputerStore) -> some View {
        List {
            Section {
                StateBanner(computer: computer)
            }
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
            if computer.overview == nil, computer.phase != .away {
                ForEach(0..<5, id: \.self) { _ in SkeletonRow() }
                    .listRowBackground(Color.clear)
                    .listRowSeparator(.hidden)
            }
            if !computer.needsYou.isEmpty {
                Section {
                    ForEach(computer.needsYou) { chat in
                        RequestCard(computer: computer, chat: chat) {
                            open(ChatRoute(hostId: computer.hostId, chatId: chat.chat.id))
                        }
                        .matchedTransitionSource(id: chat.chat.id, in: zoom)
                    }
                } header: {
                    GroupTitle(title: "Needs you", count: computer.needsYou.count)
                }
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
                .listRowInsets(EdgeInsets(top: 5, leading: 16, bottom: 5, trailing: 16))
            }
            if !computer.working.isEmpty {
                Section {
                    ForEach(computer.working) { chat in
                        row(computer, chat, line: computer.workingLine(chat.chat.id).map(workingWords))
                            .swipeActions(edge: .trailing) {
                                Button("Stop", systemImage: "stop.fill", role: .destructive) {
                                    Task { try? await computer.stopTurn(chat.chat.id) }
                                }
                            }
                    }
                } header: {
                    GroupTitle(title: "Working", count: computer.working.count)
                }
                .listRowBackground(Color.clear)
            }
            let recent = computer.recent.filter { isToday($0.chat.updatedAt) }
            if !recent.isEmpty {
                Section {
                    ForEach(recent.prefix(12)) { chat in
                        let ended = computer.recentLine(chat.chat.id)
                        row(computer, chat, line: ended?.line, outcome: ended.flatMap { outcome($0, chat) },
                            pullRequest: ended?.pullRequest)
                    }
                } header: {
                    GroupTitle(title: "Today")
                }
                .listRowBackground(Color.clear)
            }
            if computer.overview != nil, computer.needsYou.isEmpty, computer.working.isEmpty, recent.isEmpty {
                Section {
                    EmptyInbox(newWork: $newWork)
                }
                .listRowBackground(Color.clear)
                .listRowSeparator(.hidden)
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .navigationTitle(computer.hostName.isEmpty ? "Computer" : computer.hostName)
        .navigationSubtitle(subtitle(computer))
        .refreshable {
            computer.wake()
            await computer.refresh()
            await computer.refreshInbox()
        }
        .animation(.smooth, value: computer.overview?.chats.map(\.chat.status))
    }

    private func row(
        _ computer: ComputerStore, _ chat: WorkspaceChat, line: String?, outcome: String? = nil,
        pullRequest: InboxSnapshot.Recent.PullRequest? = nil
    ) -> some View {
        Button {
            open(ChatRoute(hostId: computer.hostId, chatId: chat.chat.id))
        } label: {
            ChatRow(
                chat: chat, project: computer.project(chat.projectId)?.name, line: line, outcome: outcome,
                pullRequest: pullRequest)
        }
        .buttonStyle(.plain)
        .matchedTransitionSource(id: chat.chat.id, in: zoom)
        .listRowSeparatorTint(style.line)
    }

    /// "Done in 12m · 6 files · +120 −8"; a failed turn says only how long it ran.
    private func outcome(_ ended: InboxSnapshot.Recent, _ chat: WorkspaceChat) -> String? {
        var parts: [String] = []
        if let duration = ended.durationMs {
            parts.append((chat.chat.status == .failed ? "Ran " : "Done in ") + elapsedWords(duration))
        }
        if let files = ended.files, files > 0 { parts.append(files == 1 ? "1 file" : "\(files) files") }
        if let added = ended.added, let removed = ended.removed, added + removed > 0 {
            parts.append("+\(added) −\(removed)")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    private func workingWords(_ working: InboxSnapshot.Working) -> String {
        let line = working.line ?? "Working"
        guard let since = working.since else { return line }
        return "\(line) · \(elapsedWords(Int64(Date().timeIntervalSince1970 * 1000) - since))"
    }

    private func subtitle(_ computer: ComputerStore) -> String {
        switch computer.phase {
        case .ready: "Connected"
        case .away: "Computer is away"
        case .connecting: "Connecting"
        case .ended: "Not paired"
        }
    }

    private func isToday(_ millis: Int64) -> Bool {
        Date().timeIntervalSince1970 * 1000 - Double(millis) < 86_400_000
    }
}

/// The switcher in the title's menu: every paired computer, and pairing another.
struct ComputerMenu: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        ForEach(model.computers) { computer in
            Button {
                model.select(computer.hostId)
            } label: {
                let waiting = computer.needsYou.count
                Label(
                    waiting > 0 ? "\(computer.hostName) · \(waiting) need\(waiting == 1 ? "s" : "") you" : computer.hostName,
                    systemImage: computer.hostId == model.selected?.hostId ? "checkmark" : "desktopcomputer")
            }
        }
        Divider()
        Button("Pair another computer", systemImage: "qrcode.viewfinder") { model.scanning = true }
    }
}

/// What the link to the computer is, when it is not simply connected.
struct StateBanner: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let computer: ComputerStore

    var body: some View {
        switch computer.phase {
        case .away:
            Label {
                Text("\(computer.hostName.isEmpty ? "Your computer" : computer.hostName) is not answering. Open o7 there, and check it is awake and online. This connects by itself when it is back.")
            } icon: {
                Image(systemName: "moon.zzz")
            }
            .o7Font(.meta)
            .foregroundStyle(style.fg1)
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(style.amberTint, in: .rect(cornerRadius: 16))
        case .ended(let reason, let refused):
            VStack(alignment: .leading, spacing: 12) {
                Text(reason).o7Font(.meta).foregroundStyle(style.fg1)
                if refused {
                    HStack {
                        Button("Try again") { computer.wake() }.buttonStyle(.glass)
                        Button("Forget this computer", role: .destructive) { model.discard(computer) }
                            .buttonStyle(.glass)
                    }
                }
            }
            .padding(14)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(style.redTint, in: .rect(cornerRadius: 16))
        default:
            if let problem = computer.problem {
                Text(problem).o7Font(.meta).foregroundStyle(style.red)
            } else {
                EmptyView()
            }
        }
    }
}

/// A chat that waits on the person, with its question and, for a permission, the answers.
struct RequestCard: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    let computer: ComputerStore
    let chat: WorkspaceChat
    let open: () -> Void
    @State private var answering: String?
    @State private var failed: String?

    var body: some View {
        let ask = computer.asking(chat.chat.id)?.request
        VStack(alignment: .leading, spacing: 12) {
            Button(action: open) {
                ChatRow(chat: chat, project: computer.project(chat.projectId)?.name)
            }
            .buttonStyle(.plain)
            if let ask {
                Text(ask.kind == "permission" ? "Waiting on you" : "A question for you")
                    .o7Font(.meta)
                    .foregroundStyle(style.accent)
                InlineCode(text: ask.title)
                if ask.kind == "permission", let options = ask.options, !options.isEmpty {
                    // Side by side at the usual text sizes; one per row once text grows, so no
                    // answer is ever cut short.
                    if dynamicTypeSize <= .large {
                        ViewThatFits(in: .horizontal) {
                            HStack(spacing: 8) { choices(ask, options, fit: true) }
                            VStack(spacing: 8) { choices(ask, options, fit: false) }
                        }
                    } else {
                        VStack(spacing: 8) { choices(ask, options, fit: false) }
                    }
                } else {
                    Button("Answer in the chat", action: open)
                        .o7Font(.ui(14, weight: .medium))
                        .buttonStyle(.glass)
                }
            }
            if let failed {
                Text(failed).o7Font(.meta).foregroundStyle(style.red)
            }
        }
        .padding(14)
        .background(style.raised, in: .rect(cornerRadius: 18))
        .overlay(RoundedRectangle(cornerRadius: 18).strokeBorder(style.accent.opacity(0.35), lineWidth: 1))
        .sensoryFeedback(.impact(weight: .medium), trigger: answering)
    }

    private func choices(_ ask: InboxSnapshot.Request, _ options: [PermissionOption], fit: Bool) -> some View {
        ForEach(options.prefix(4)) { option in
            Button {
                Task { await answer(ask, option.optionId) }
            } label: {
                Group {
                    if answering == option.optionId {
                        ProgressView()
                    } else {
                        Text(option.label)
                            .multilineTextAlignment(.center)
                            .fixedSize(horizontal: fit, vertical: true)
                    }
                }
                .o7Font(.ui(14, weight: .medium))
                .frame(maxWidth: .infinity)
                .foregroundStyle(option.kind.allows ? style.onPrimary : style.fg1)
            }
            .buttonStyle(.glassProminent)
            .tint(option.kind.allows ? style.primary : style.control)
            .disabled(answering != nil)
        }
    }

    private func answer(_ ask: InboxSnapshot.Request, _ option: String) async {
        let allows = ask.options?.first { $0.optionId == option }?.kind.allows ?? false
        if allows, model.settings.faceIdToApprove, !(await Authenticator.confirm("Allow what the agent asks")) {
            return
        }
        answering = option
        defer { answering = nil }
        do {
            try await computer.allow(chat.chat.id, request: ask.id, option: option)
            failed = nil
        } catch {
            failed = (error as? LocalizedError)?.errorDescription ?? "Not sent."
        }
    }
}

/// A command or a path, set as code.
struct InlineCode: View {
    @Environment(\.style) private var style
    let text: String
    var body: some View {
        Text(text.replacingOccurrences(of: "`", with: ""))
            .o7Font(.code)
            .foregroundStyle(style.codeText)
            .lineLimit(4)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(style.codeBackground, in: .rect(cornerRadius: 10))
    }
}

/// A row while the first overview is on its way.
struct SkeletonRow: View {
    @Environment(\.style) private var style
    var body: some View {
        HStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 7).fill(style.control).frame(width: 30, height: 30)
            VStack(alignment: .leading, spacing: 6) {
                RoundedRectangle(cornerRadius: 4).fill(style.control).frame(width: 190, height: 12)
                RoundedRectangle(cornerRadius: 4).fill(style.card).frame(width: 120, height: 10)
            }
        }
        .padding(.vertical, 12)
        .phaseAnimator([0.45, 1]) { content, value in content.opacity(value) } animation: { _ in
            .easeInOut(duration: 1.2)
        }
        .accessibilityHidden(true)
    }
}

struct EmptyInbox: View {
    @Environment(\.style) private var style
    @Binding var newWork: Bool
    var body: some View {
        VStack(spacing: 10) {
            Text("Nothing needs you")
                .o7Font(.heading)
                .foregroundStyle(style.fg1)
            Text("No agent is working or waiting. Start one from here.")
                .o7Font(.meta)
                .foregroundStyle(style.fg3)
                .multilineTextAlignment(.center)
            Button("New chat", systemImage: "plus") { newWork = true }
                .buttonStyle(.glass)
                .padding(.top, 6)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 48)
    }
}

/// Face ID, or the passcode, before something sensitive.
enum Authenticator {
    static func confirm(_ reason: String) async -> Bool {
        let context = LAContext()
        guard context.canEvaluatePolicy(.deviceOwnerAuthentication, error: nil) else { return true }
        return (try? await context.evaluatePolicy(.deviceOwnerAuthentication, localizedReason: reason)) ?? false
    }
}
