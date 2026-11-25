import O7Client
import O7Marks
import O7Wire
import SwiftUI

/// The chat's model and thinking level, which apply as they are chosen, and the agent: choosing
/// another hands the chat over, into a new chat that starts from this one's history.
struct PickersSheet: View {
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    let computer: ComputerStore
    let chat: ChatSummary
    let running: Bool
    let switched: (String) -> Void

    @State private var choices: Choices?
    @State private var said: String?
    @State private var failure: String?
    @State private var agent: String?
    @State private var switching = false

    var body: some View {
        NavigationStack {
            Form {
                if let choices {
                    if !choices.models.isEmpty {
                        Picker("Model", selection: binding(choices.model, set: setModel)) {
                            if choices.model == nil { Text("The agent's default").tag("") }
                            ForEach(choices.models) { Text($0.name).tag($0.value) }
                        }
                    }
                    if !choices.thinking.isEmpty {
                        Picker("Thinking", selection: binding(choices.thinkingLevel, set: setThinking)) {
                            if choices.thinkingLevel == nil { Text("Default").tag("") }
                            ForEach(choices.thinking) { Text($0.name).tag($0.value) }
                        }
                    }
                    if choices.models.isEmpty, choices.thinking.isEmpty {
                        Text("\(chat.agentName) offers no choice of model here. It may list them once it has started.")
                            .o7Font(.meta)
                            .foregroundStyle(style.fg3)
                    }
                } else if failure == nil {
                    HStack {
                        PixelLoader(size: 14, tempo: .thinking)
                        Text("Asking the computer.").foregroundStyle(style.fg3)
                    }
                }
                Section {
                    ForEach((computer.overview?.agents ?? []).filter { $0.available || $0.id == chat.agentId }) { option in
                        Button {
                            agent = option.id
                        } label: {
                            HStack(spacing: 10) {
                                AgentMark(agent: option.id, size: 24)
                                Text(option.name + (option.id == chat.agentId ? " (now)" : ""))
                                    .foregroundStyle(style.fg1)
                                Spacer()
                                if (agent ?? chat.agentId) == option.id {
                                    Image(systemName: "checkmark").foregroundStyle(style.accent)
                                }
                            }
                        }
                    }
                } header: {
                    Text("Agent")
                } footer: {
                    if let agent, agent != chat.agentId {
                        let name = computer.agentName(agent)
                        Text(
                            running
                                ? "\(chat.agentName) is still working. Stop the turn, or wait for it, then switch."
                                : "The chat carries on with \(name) in a new chat that starts from this one's history.")
                    }
                }
                if let agent, agent != chat.agentId {
                    Button {
                        Task { await switchAgent(agent) }
                    } label: {
                        HStack {
                            Text(switching ? "Switching" : "Switch to \(computer.agentName(agent))")
                            if switching { Spacer(); ProgressView() }
                        }
                    }
                    .disabled(running || switching)
                }
                if let said {
                    Text(said).o7Font(.meta).foregroundStyle(style.fg3)
                }
                if let failure {
                    Text(failure).o7Font(.meta).foregroundStyle(style.red)
                }
            }
            .o7Font(.ui(16))
            .scrollContentBackground(.hidden)
            .background(style.pane.ignoresSafeArea())
            .navigationTitle("Model and agent")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
            .task { await load() }
            .sensoryFeedback(.selection, trigger: said)
        }
    }

    private func binding(_ value: String?, set: @escaping (String) -> Void) -> Binding<String> {
        Binding(get: { value ?? "" }, set: { if !$0.isEmpty { set($0) } })
    }

    private func load() async {
        do {
            choices = try await computer.connection.send(.options(chat.id), as: Choices.self)
        } catch {
            failure = (error as? LocalizedError)?.errorDescription ?? "The computer did not answer."
        }
    }

    private func setModel(_ value: String) {
        choices?.model = value
        Task {
            do {
                try await computer.connection.perform(.setModel(chat.id, model: value))
                await load()
                said = "Model set to \(choices?.models.first { $0.value == value }?.name ?? value)."
            } catch {
                failure = (error as? LocalizedError)?.errorDescription
            }
        }
    }

    private func setThinking(_ value: String) {
        choices?.thinkingLevel = value
        Task {
            do {
                try await computer.connection.perform(.setThinking(chat.id, level: value))
                await load()
                said = "Thinking set to \(choices?.thinking.first { $0.value == value }?.name ?? value)."
            } catch {
                failure = (error as? LocalizedError)?.errorDescription
            }
        }
    }

    private func switchAgent(_ agentId: String) async {
        switching = true
        defer { switching = false }
        do {
            let next = try await computer.connection.send(.switchAgent(chat.id, agentId: agentId), as: Switched.self, timeout: .slow)
            await computer.refresh()
            dismiss()
            switched(next.chatId)
        } catch {
            failure = (error as? LocalizedError)?.errorDescription ?? "The agent did not take the chat."
        }
    }
}
