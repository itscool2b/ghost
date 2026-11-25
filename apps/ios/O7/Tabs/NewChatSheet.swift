import O7Marks
import O7Wire
import SwiftUI

/// Starts work: what should happen, where, with which agent, and whether it is a task on its own
/// branch, reviewed when done, started now or tonight.
struct NewChatSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    var projectId: String?
    /// Opened from Tonight: a task, queued for the night.
    var tonight = false
    let opened: (ChatRoute) -> Void

    @State private var text = ""
    @State private var photos: [PickedPhoto] = []
    @State private var picking = false
    @State private var shooting = false
    @State private var folderId: String?
    @State private var agentId: String?
    @State private var ownBranch = false
    @State private var review = true
    @State private var later = false
    @State private var access = "auto_edit"
    @State private var busy = false
    @State private var failure: String?
    @State private var queuedWords: String?
    @FocusState private var focused: Bool

    var body: some View {
        NavigationStack {
            if let computer = model.selected, let overview = computer.overview {
                form(computer, overview)
            } else {
                ContentUnavailableView(
                    "Waiting for your computer", systemImage: "desktopcomputer",
                    description: Text("New work starts once it is connected."))
            }
        }
        .onAppear {
            if tonight {
                ownBranch = true
                later = true
            }
        }
    }

    /// A task always has its own branch; tonight is when it starts.
    private var asTask: Bool { ownBranch }

    private func form(_ computer: ComputerStore, _ overview: Overview) -> some View {
        let folders = overview.projects.flatMap { project in
            project.folders.map { (project: project, folder: $0) }
        }
        let chosenFolder =
            folderId ?? folders.first(where: { $0.project.id == projectId })?.folder.id
            ?? folders.first?.folder.id
        let project = folders.first { $0.folder.id == chosenFolder }?.project
        let usual = project?.defaultAgent ?? overview.defaultAgent
        let agent = agentId ?? usual
        let reviewer = overview.agents.first { $0.available && $0.id != agent }
        let empty = text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        return ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                VStack(alignment: .leading, spacing: 0) {
                    TextField("What should happen?", text: $text, axis: .vertical)
                        .o7Font(.ui(18, relativeTo: .body))
                        .foregroundStyle(style.fg1)
                        .lineLimit(4...12)
                        .focused($focused)
                        .padding(16)
                    if !photos.isEmpty {
                        PhotoStrip(photos: $photos).padding(.bottom, 10)
                    }
                    if computer.offersPhotos {
                        HStack {
                            PhotoButton(picking: $picking, shooting: $shooting)
                                .disabled(photos.count >= photoLimit)
                            Spacer()
                        }
                        .padding([.horizontal, .bottom], 10)
                    }
                }
                .background(style.card, in: .rect(cornerRadius: 18))
                field("Project") {
                    Menu {
                        ForEach(folders, id: \.folder.id) { entry in
                            Button(label(entry.project, entry.folder)) { folderId = entry.folder.id }
                        }
                    } label: {
                        HStack {
                            Image(systemName: "folder")
                            Text(
                                folders.first { $0.folder.id == chosenFolder }.map { label($0.project, $0.folder) }
                                    ?? "Choose")
                            Spacer()
                            Image(systemName: "chevron.up.chevron.down").font(.caption)
                        }
                        .o7Font(.ui(15))
                        .foregroundStyle(style.fg1)
                        .padding(14)
                        .frame(minHeight: 44)
                        .background(style.card, in: .rect(cornerRadius: 14))
                    }
                }
                field("Agent") {
                    ScrollView(.horizontal, showsIndicators: false) {
                        HStack(spacing: 8) {
                            ForEach(overview.agents.filter(\.available)) { choice in
                                let chosen = agent == choice.id
                                Button {
                                    agentId = choice.id
                                } label: {
                                    HStack(spacing: 8) {
                                        AgentMark(agent: choice.id, size: 22)
                                        Text(choice.name + (choice.id == usual ? " (usual)" : ""))
                                            .o7Font(.ui(14, weight: .medium))
                                    }
                                    .foregroundStyle(chosen ? style.onPrimary : style.fg1)
                                    .padding(.horizontal, 12)
                                    .padding(.vertical, 8)
                                }
                                .buttonStyle(.glassProminent)
                                .tint(chosen ? style.primary : style.control)
                                .accessibilityAddTraits(chosen ? .isSelected : [])
                            }
                        }
                    }
                }
                if computer.offersTasks {
                    VStack(spacing: 0) {
                        Toggle(isOn: $ownBranch.animation(.snappy)) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text("Own branch").o7Font(.rowTitle).foregroundStyle(style.fg1)
                                Text("A task in a worktree of its own, so your checkout stays as it is.")
                                    .o7Font(.meta).foregroundStyle(style.fg3)
                            }
                        }
                        .padding(14)
                        if asTask {
                            Divider().padding(.leading, 14)
                            Toggle(isOn: $review) {
                                VStack(alignment: .leading, spacing: 2) {
                                    Text("Review when done").o7Font(.rowTitle).foregroundStyle(style.fg1)
                                    Text(
                                        reviewer.map { "\($0.name) reads the change and says what it finds." }
                                            ?? "Another agent reads the change and says what it finds."
                                    )
                                    .o7Font(.meta).foregroundStyle(style.fg3)
                                }
                            }
                            .padding(14)
                            Divider().padding(.leading, 14)
                            HStack {
                                Text("When").o7Font(.rowTitle).foregroundStyle(style.fg1)
                                Spacer()
                                Picker("When", selection: $later.animation(.snappy)) {
                                    Text("Now").tag(false)
                                    Text("Tonight").tag(true)
                                }
                                .pickerStyle(.segmented)
                                .fixedSize()
                            }
                            .padding(14)
                            Divider().padding(.leading, 14)
                            HStack {
                                Text("Access").o7Font(.rowTitle).foregroundStyle(style.fg1)
                                Spacer()
                                Picker("Access", selection: $access) {
                                    Text("Ask first").tag("ask")
                                    Text("Edit files").tag("auto_edit")
                                    Text("Full access").tag("full_auto")
                                }
                                .pickerStyle(.menu)
                            }
                            .padding(.horizontal, 14)
                            .padding(.vertical, 6)
                            if later {
                                Text(
                                    "o7 starts it between 1 and 6 in the morning on \(computer.hostName)'s clock. The computer must be awake."
                                )
                                .o7Font(.meta)
                                .foregroundStyle(style.fg3)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .padding([.horizontal, .bottom], 14)
                            }
                        }
                    }
                    .tint(style.primary)
                    .background(style.card, in: .rect(cornerRadius: 18))
                }
                if let failure {
                    Text(failure).o7Font(.meta).foregroundStyle(style.red)
                }
            }
            .padding(20)
        }
        .background(style.pane.ignoresSafeArea())
        .photoSources($photos, picking: $picking, shooting: $shooting)
        .navigationTitle(tonight ? "Queue a task" : "New work")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItem(placement: .cancellationAction) {
                Button("Cancel", systemImage: "xmark") { dismiss() }
            }
            ToolbarItem(placement: .confirmationAction) {
                Button {
                    Task {
                        await start(
                            computer, folder: chosenFolder, projectId: project?.id,
                            agent: agentId == usual ? nil : agentId)
                    }
                } label: {
                    if busy {
                        ProgressView()
                    } else {
                        Text(asTask && later ? "Queue" : "Start")
                    }
                }
                .buttonStyle(.glassProminent)
                .disabled(busy || chosenFolder == nil || empty)
                .accessibilityLabel(asTask && later ? "Queue for tonight" : "Start")
            }
        }
        .alert("Queued for tonight", isPresented: .constant(queuedWords != nil)) {
            Button("Done") {
                queuedWords = nil
                dismiss()
            }
        } message: {
            Text(queuedWords ?? "")
        }
        .onAppear { focused = true }
        .onChange(of: ownBranch) { _, own in
            if !own { later = false }
        }
        .sensoryFeedback(.success, trigger: queuedWords) { _, new in new != nil }
    }

    private func field(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).o7Font(.meta).foregroundStyle(style.fg3)
            content()
        }
    }

    private func label(_ project: ProjectSummary, _ folder: ProjectFolder) -> String {
        project.name == folder.displayName ? project.name : "\(project.name) · \(folder.displayName)"
    }

    private func start(_ computer: ComputerStore, folder: String?, projectId: String?, agent: String?) async {
        guard let folder else { return }
        busy = true
        defer { busy = false }
        let goal = text.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            if asTask {
                let started = try await computer.startTask(
                    goal: goal, projectId: projectId, agentId: agent, review: review, tonight: later,
                    access: access, photos: photos)
                if let chatId = started.chatId {
                    dismiss()
                    opened(ChatRoute(hostId: computer.hostId, chatId: chatId))
                } else if tonight {
                    // Opened from Tonight: the list there shows it.
                    dismiss()
                } else {
                    queuedWords = "It starts tonight. Tonight lists it, and you can cancel it there until it starts."
                }
            } else {
                let chatId = try await computer.create(folderId: folder, agentId: agent, text: goal, photos: photos)
                dismiss()
                opened(ChatRoute(hostId: computer.hostId, chatId: chatId))
            }
        } catch {
            failure = (error as? LocalizedError)?.errorDescription ?? "It did not start."
        }
    }
}
