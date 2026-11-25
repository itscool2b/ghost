public import O7Wire

// A question card's grouped questions while they are answered (`questions.ts`). Unsubmitted input
// stays local to the card. No option is selected by default.

public struct QuestionDraft: Sendable, Hashable, Codable {
    public var selected: [Int]
    public var text: String

    public init(selected: [Int] = [], text: String = "") {
        self.selected = selected
        self.text = text
    }
}

public let emptyQuestionDraft = QuestionDraft()

/// Radio choices replace free text; checkbox choices preserve the other selected answers.
public func chooseQuestionOption(_ question: UiQuestion, _ draft: QuestionDraft, _ index: Int) -> QuestionDraft {
    if index < 0 || index >= question.options.count { return draft }
    if !question.multiSelect { return QuestionDraft(selected: [index], text: "") }
    var next = draft
    next.selected =
        draft.selected.contains(index) ? draft.selected.filter { $0 != index } : (draft.selected + [index]).sorted()
    return next
}

/// A single-select free-text answer replaces its radio choice.
public func writeQuestionText(_ question: UiQuestion, _ draft: QuestionDraft, _ text: String) -> QuestionDraft {
    if !question.allowFreeText { return draft }
    return QuestionDraft(selected: question.multiSelect ? draft.selected : [], text: text)
}

/// Form validation only; Rust validates the submitted answer against the pending request too.
/// Drafts are looked up by question id.
public func questionsAnswer(_ questions: [UiQuestion], _ drafts: [String: QuestionDraft]) -> UiAnswer? {
    if questions.isEmpty || ExactSet(questions.map(\.id)).count != questions.count { return nil }
    var answers: [UiQuestionAnswer] = []
    for question in questions {
        if question.id.isEmpty { return nil }
        let draft = drafts[question.id] ?? emptyQuestionDraft
        let text: String? = JS.trim(draft.text.units).isEmpty ? nil : draft.text
        if text != nil && !question.allowFreeText { return nil }
        if Set(draft.selected).count != draft.selected.count { return nil }
        if draft.selected.contains(where: { $0 < 0 || $0 >= question.options.count }) { return nil }
        let count = draft.selected.count + (text == nil ? 0 : 1)
        if count == 0 || (!question.multiSelect && count != 1) { return nil }
        answers.append(UiQuestionAnswer(questionId: question.id, selected: draft.selected.sorted(), text: text))
    }
    return .questions(answers: answers)
}
