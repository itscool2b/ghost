import O7Marks
import O7Transcript
import O7Wire
import SwiftUI

/// What an agent asks before a tool runs. Waiting: one raised card, "Waiting on you", the command or
/// file in mono, and the agent's own choices. Answered: one line.
struct PermissionCard: View {
    @Environment(AppModel.self) private var model
    @Environment(\.style) private var style
    let card: PermissionItem
    let chat: ChatStore
    @State private var answering: String?

    var body: some View {
        if let resolution = card.resolution {
            Text("\(card.request.title.replacingOccurrences(of: "`", with: "")): \(words(resolution))")
                .o7Font(.meta)
                .foregroundStyle(style.fg3)
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            VStack(alignment: .leading, spacing: 10) {
                HStack(spacing: 8) {
                    Circle().fill(style.accent).frame(width: 8, height: 8)
                    Text("Waiting on you").o7Font(.ui(13, weight: .medium)).foregroundStyle(style.accent)
                }
                InlineCode(text: card.request.title)
                if card.request.options.isEmpty {
                    choice(nil, label: "Decline", prominent: false)
                } else {
                    VStack(spacing: 8) {
                        ForEach(card.request.options) { option in
                            choice(option.optionId, label: option.label, prominent: option.kind.allows)
                        }
                    }
                }
            }
            .padding(14)
            .background(style.raised, in: .rect(cornerRadius: 18))
            .overlay(RoundedRectangle(cornerRadius: 18).strokeBorder(style.accent.opacity(0.35), lineWidth: 1))
            .accessibilityElement(children: .contain)
            .accessibilityLabel("The agent asks for permission")
        }
    }

    private func choice(_ option: String?, label: String, prominent: Bool) -> some View {
        Button {
            Task {
                let allows = card.request.options.first { $0.optionId == option }?.kind.allows ?? false
                if allows, model.settings.faceIdToApprove, !(await Authenticator.confirm("Allow what the agent asks")) {
                    return
                }
                answering = option ?? "decline"
                await chat.permission(card.request.requestId, option: option)
                answering = nil
            }
        } label: {
            HStack {
                if answering == (option ?? "decline") { ProgressView() }
                Text(label)
            }
            .o7Font(.ui(15, weight: .medium))
            .foregroundStyle(prominent ? style.onPrimary : style.fg1)
            .frame(maxWidth: .infinity, minHeight: 28)
        }
        .buttonStyle(.glassProminent)
        .tint(prominent ? style.primary : style.control)
        .disabled(answering != nil)
        .sensoryFeedback(.impact(weight: .medium), trigger: answering)
    }

    private func words(_ resolution: PermissionResolution) -> String {
        switch resolution.decision {
        case .selected(let id, _):
            card.request.options.first { $0.optionId == id }?.label ?? "answered"
        case .cancelled:
            "declined"
        }
    }
}

/// A question an agent or a tool asks: yes or no, one of several, words, or a set of questions.
struct QuestionCard: View {
    @Environment(\.style) private var style
    let card: QuestionItem
    let chat: ChatStore
    @State private var text = ""
    @State private var drafts: [String: QuestionDraft] = [:]
    @State private var busy = false

    var body: some View {
        let prompt = card.request.prompt
        if case .notify(let message) = prompt {
            Text(message).o7Font(.meta).foregroundStyle(style.fg3)
        } else if card.resolution != nil {
            Text("\(prompt.message): answered").o7Font(.meta).foregroundStyle(style.fg3)
        } else {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 8) {
                    Circle().fill(style.accent).frame(width: 8, height: 8)
                    Text("Waiting on you").o7Font(.ui(13, weight: .medium)).foregroundStyle(style.accent)
                }
                Text(prompt.message).o7Font(.prose).foregroundStyle(style.fg1)
                controls(prompt)
                Button("Dismiss") { send(nil) }
                    .o7Font(.ui(14))
                    .foregroundStyle(style.fg3)
                    .buttonStyle(.plain)
                    .disabled(busy)
            }
            .padding(14)
            .background(style.raised, in: .rect(cornerRadius: 18))
            .overlay(RoundedRectangle(cornerRadius: 18).strokeBorder(style.accent.opacity(0.35), lineWidth: 1))
        }
    }

    @ViewBuilder private func controls(_ prompt: UiPrompt) -> some View {
        switch prompt {
        case .confirm:
            HStack(spacing: 8) {
                answerButton("Yes", prominent: true) { send(.confirm(yes: true)) }
                answerButton("No", prominent: false) { send(.confirm(yes: false)) }
            }
        case .select(_, let options):
            VStack(spacing: 8) {
                ForEach(Array(options.enumerated()), id: \.offset) { index, option in
                    answerButton(option, prominent: false) { send(.select(index: index)) }
                }
            }
        case .input(_, let placeholder):
            TextField(placeholder ?? "Your answer", text: $text, axis: .vertical)
                .o7Font(.composer)
                .padding(12)
                .background(style.control, in: .rect(cornerRadius: 12))
            answerButton("Answer", prominent: true) { send(.input(text: text)) }
                .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        case .questions(_, let questions):
            ForEach(questions) { question in
                VStack(alignment: .leading, spacing: 8) {
                    Text(question.question).o7Font(.ui(15, weight: .medium)).foregroundStyle(style.fg1)
                    ForEach(Array(question.options.enumerated()), id: \.offset) { index, option in
                        let chosen = drafts[question.id, default: emptyQuestionDraft].selected.contains(index)
                        Button {
                            drafts[question.id] = chooseQuestionOption(question, drafts[question.id, default: emptyQuestionDraft], index)
                        } label: {
                            HStack(alignment: .top, spacing: 10) {
                                Image(systemName: question.multiSelect
                                    ? (chosen ? "checkmark.square.fill" : "square")
                                    : (chosen ? "largecircle.fill.circle" : "circle"))
                                    .foregroundStyle(chosen ? style.accent : style.fg3)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(option.label).o7Font(.chat).foregroundStyle(style.fg1)
                                    if !option.description.isEmpty {
                                        Text(option.description).o7Font(.small).foregroundStyle(style.fg3)
                                    }
                                }
                                Spacer(minLength: 0)
                            }
                            .contentShape(.rect)
                        }
                        .buttonStyle(.plain)
                    }
                    if question.allowFreeText {
                        TextField("Something else", text: Binding(
                            get: { drafts[question.id, default: emptyQuestionDraft].text },
                            set: { drafts[question.id] = writeQuestionText(question, drafts[question.id, default: emptyQuestionDraft], $0) }))
                            .o7Font(.chat)
                            .padding(10)
                            .background(style.control, in: .rect(cornerRadius: 10))
                    }
                }
            }
            let answer = questionsAnswer(questions, drafts)
            answerButton("Answer", prominent: true) { send(answer) }
                .disabled(answer == nil)
        case .form, .link, .unknown:
            Text("Answer this one on your computer, or dismiss it here.")
                .o7Font(.meta)
                .foregroundStyle(style.fg3)
        case .notify:
            EmptyView()
        }
    }

    private func answerButton(_ title: String, prominent: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .o7Font(.ui(15, weight: .medium))
                .foregroundStyle(prominent ? style.onPrimary : style.fg1)
                .frame(maxWidth: .infinity, minHeight: 28)
        }
        .buttonStyle(.glassProminent)
        .tint(prominent ? style.primary : style.control)
        .disabled(busy)
    }

    private func send(_ answer: UiAnswer?) {
        busy = true
        Task {
            await chat.answer(card.request.requestId, answer)
            busy = false
        }
    }
}
