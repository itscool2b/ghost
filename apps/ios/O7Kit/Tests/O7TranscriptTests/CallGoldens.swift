import Foundation
import O7Wire

@testable import O7Transcript

// Runs a "calls" golden's calls on the port: each TypeScript function by its name, its arguments
// read from the JSON the writer gave them, its result written as the writer writes the
// TypeScript's (`enc` in write-goldens.mts).

struct CallsGolden: Decodable {
    struct Call: Decodable {
        let fn: String
        let args: [JSONValue]
        let out: JSONValue
    }
    let name: String
    let calls: [Call]
}

private func zone(_ name: JSONValue) throws -> TimeZone {
    let identifier = try name.text
    guard let zone = TimeZone(identifier: identifier) else { throw GoldenError("no time zone \(identifier)") }
    return zone
}

private func formatter(_ name: String) -> (Double) -> String {
    switch name {
    case "formatElapsed": formatElapsed
    case "formatDuration": formatDuration
    default: { "\(JS.numberString($0 / 1000)) s" }
    }
}

private func json(_ draft: FieldDraft?) throws -> JSONValue {
    try draft.map(JSONValue.golden) ?? .null
}

private func json(_ answer: FieldAnswer) -> JSONValue {
    .object(["value": answer.value?.json ?? .null, "error": .golden(answer.error)])
}

/// The port's answer to one call, as JSON.
func call(_ fn: String, _ a: [JSONValue]) throws -> JSONValue {
    switch fn {
    // lib/format.ts
    case "formatDuration": return .string(formatDuration(try a[0].double))
    case "formatCount": return .string(formatCount(try a[0].double))
    case "formatCost": return .golden(formatCost(try a[0].optionalDouble, try a[1].text))

    // chat/working.ts
    case "formatElapsed": return .string(formatElapsed(try a[0].double))
    case "seedOf": return .number(Double(seedOf(try a[0].text)))
    case "workingVerb":
        return .string(
            workingVerb(
                seed: UInt32(try a[0].double), tick: try a[1].int,
                context: try a[2].optional(WorkingContext.self)))
    case "rotationDelay": return .number(Double(rotationDelay(seed: UInt32(try a[0].double), tick: try a[1].int)))
    case "workingContext": return .golden(workingContext(try a[0].optional(ToolKind.self))?.rawValue)
    case "verbs":
        return .object([
            "thinking": .array(thinkingVerbs.map(JSONValue.string)),
            "looking": .array(lookingVerbs.map(JSONValue.string)),
            "making": .array(makingVerbs.map(JSONValue.string)),
            "running": .array(runningVerbs.map(JSONValue.string)),
            "min": .number(Double(rotationMinMs)),
            "max": .number(Double(rotationMaxMs)),
        ])
    case "workingSince":
        var memory = WorkingSince()
        return .array(
            try a[0].items.map { pair in
                let parts = try pair.items
                return .number(memory.since(try parts[0].text, now: try parts[1].double))
            })

    // chat/handoff.ts
    case "parseHandoff": return try parseHandoff(try a[0].text).map(JSONValue.golden) ?? .null
    case "splitHandoff": return try splitHandoff(try a[0].text).map(JSONValue.golden) ?? .null
    case "PROMPT_MARK": return .string(handoffPromptMark)

    // chat/memoryContext.ts
    case "splitMemoryContext": return try .golden(splitMemoryContext(try a[0].text))

    // chat/tasks.ts
    case "newestTurnKey": return .golden(newestTurnKey(try a[0].arg([Segment].self)))
    case "stripTasks":
        return try .golden(
            stripTasks(
                try a[0].map(TaskItem.self), turnKey: try a[1].optional(String.self), live: try a[2].flag,
                liveTasks: try a[3].stringSet))
    case "childTasks": return try .golden(childTasks(try a[0].map(TaskItem.self), parentId: try a[1].text))
    case "shownTasks": return try .golden(shownTasks(try a[0].map(TaskItem.self)))
    case "taskDisplay":
        return .string(
            taskDisplay(try a[0].arg(TaskItem.self), live: try a[1].flag, liveTasks: try a[2].stringSet).rawValue)
    case "tasksByToolCall": return .goldenMap(tasksByToolCall(try a[0].map(TaskItem.self))) { .string($0) }
    case "mergeTask":
        return try .golden(
            mergeTask(
                try a[0].optional(TaskItem.self), try a[1].arg(BackgroundTask.self), order: Int64(try a[2].double),
                turnKey: try a[3].optional(String.self)))
    case "withStep":
        return try .golden(
            withStep(
                try a[0].optional(TaskItem.self), taskId: try a[1].text, step: try a[2].arg(TaskStep.self),
                order: Int64(try a[3].double)))
    case "taskElapsed":
        return .golden(
            taskElapsed(try a[0].arg(BackgroundTask.self), display: try a[1].arg(TaskStatus.self), now: try a[2].double))
    case "compactElapsed": return .golden(compactElapsed(try a[0].optionalDouble))
    case "elapsedLabel": return .golden(elapsedLabel(try a[0].optionalDouble))
    case "toolUsesLabel": return .golden(toolUsesLabel(try a[0].optionalDouble.map { Int($0) }))
    case "taskTokensLabel": return .golden(taskTokensLabel(try a[0].optionalDouble.map { Int64($0) }))
    case "taskStatusLabel": return .string(taskStatusLabel(try a[0].arg(TaskStatus.self)))
    case "endedAnnouncement":
        return .golden(endedAnnouncement(try a[0].arg(BackgroundTask.self), display: try a[1].arg(TaskStatus.self)))
    case "repeatsSummary":
        return .bool(repeatsSummary(try a[0].optional(TaskStep.self), summary: try a[1].optional(String.self)))

    // chat/transcript.ts
    case "steeringState": return .golden(steeringState(try a[0].arg(UserItem.self), settled: try a[1].flag)?.rawValue)
    case "toolDisplayStatus":
        return .string(
            toolDisplayStatus(try a[0].arg(ToolItem.self), end: try a[1].optional(TurnEnd.self), live: try a[2].flag).rawValue)
    case "hasProcess": return .bool(hasProcess(try a[0].arg(SessionStatus.self)))
    case "stopReasonLabel": return .string(stopReasonLabel(try a[0].arg(TurnEnd.self)))
    case "shownThought": return .bool(shownThought(try a[0].arg(ThoughtItem.self), thinking: try a[1].flag))
    case "UNANNOUNCED_TOOL_TITLE": return .string(unannouncedToolTitle)

    // chat/toolWords.ts
    case "toolPhrase":
        return try .golden(
            toolPhrase(try a[0].text, kind: try a[1].arg(ToolKind.self), display: try a[2].arg(ToolDisplayStatus.self)))
    case "parseTitle": return try .golden(parseTitle(try a[0].text, kind: try a[1].arg(ToolKind.self)))
    case "toolsSummary": return .string(toolsSummary(try a[0].arg([GroupedTool].self)))
    case "groupResult": return try .golden(groupResult(try a[0].arg([GroupedTool].self)))

    // chat/toolDetails.ts
    case "toolInput": return try .golden(toolInput(try a[0].arg(ToolItem.self)))
    case "prettyJson": return .golden(prettyJson(try a[0].text))
    case "searchLinks": return try .golden(searchLinks(try a[0].arg(ToolItem.self)))
    case "withoutSearchLinks": return .string(withoutSearchLinks(try a[0].text))
    case "diffDelta": return try diffDelta(try a[0].arg(ToolItem.self)).map(JSONValue.golden) ?? .null

    // chat/workFold.ts
    case "workFold": return try workFold(try a[0].arg(RenderedSegment.self)).map(JSONValue.golden) ?? .null
    case "changedFiles": return .array(changedFiles(try a[0].arg(RenderedSegment.self)).map(JSONValue.string))
    case "workedLabel":
        return .string(
            workedLabel(
                steps: try a[0].optionalDouble.map { Int($0) }, durationMs: try a[1].optionalDouble.map { Int64($0) },
                format: formatter(try a[2].text)))

    // chat/present.ts
    case "statusWord": return .string(statusWord(try a[0].arg(SessionStatus.self)))
    case "folderStateLabel": return .golden(folderStateLabel(try a[0].arg(FolderState.self)))
    case "autonomyLabel": return .string(autonomyLabel(try a[0].arg(Autonomy.self)))
    case "AUTONOMIES": return .array(autonomies.map { .string($0.rawValue) })
    case "permissionResolutionLabel":
        return .string(
            permissionResolutionLabel(try a[0].arg(PermissionResolution.self), options: try a[1].arg([PermissionOption].self)))
    case "answerLabel": return .string(answerLabel(try a[0].arg(UiAnswer.self), prompt: try a[1].arg(UiPrompt.self)))
    case "questionResolutionLabel":
        return .string(questionResolutionLabel(try a[0].arg(QuestionResolution.self), prompt: try a[1].arg(UiPrompt.self)))
    case "isStartFailure": return .bool(isStartFailure(try a[0].arg(ErrorKind.self)))
    case "errorActions":
        let context = a[1]
        return try .golden(
            errorActions(
                try a[0].arg(ErrorKind.self), newest: try context["newest"]?.flag ?? false,
                live: try context["live"]?.flag ?? false, resumable: try context["resumable"]?.flag ?? false,
                recreatable: context["recreatable"]?.bool))
    case "authCommandText": return .string(authCommandText(try a[0].arg(AuthCommand.self)))
    case "tokensLabel": return .string(tokensLabel(try a[0].arg(TokenUsage.self)))
    case "contextLabel": return .golden(contextLabel(try a[0].optional(UsageInfo.self)))
    case "costLabel": return .golden(costLabel(try a[0].optional(UsageInfo.self)))
    case "orderedOptions": return try .golden(orderedOptions(try a[0].arg([ConfigOption].self)))
    case "pendingLabel": return .string(pendingLabel(try a[0].arg(ConfigOption.self), try a[1].arg(ConfigValue.self)))
    case "levelName": return .string(levelName(value: try a[0]["value"]?.text ?? "", name: try a[0]["name"]?.text ?? ""))
    case "levelShortName": return .string(levelShortName(try a[0].text))
    case "slashQuery": return .golden(slashQuery(try a[0].text))
    case "minutesLabel": return .string(minutesLabel(try a[0].int))
    case "matchingCommands":
        return try .golden(matchingCommands(try a[0].arg([SlashCommand].self), query: try a[1].text))

    // chat/questions.ts
    case "questionsAnswer":
        let drafts = try a[1].map(QuestionDraft.self)
        return try questionsAnswer(try a[0].arg([UiQuestion].self), Dictionary(drafts.map { ($0.key, $0.value) }) { $1 })
            .map(JSONValue.golden) ?? .null
    case "chooseQuestionOption":
        return try .golden(
            chooseQuestionOption(try a[0].arg(UiQuestion.self), try a[1].arg(QuestionDraft.self), try a[2].int))
    case "writeQuestionText":
        return try .golden(writeQuestionText(try a[0].arg(UiQuestion.self), try a[1].arg(QuestionDraft.self), try a[2].text))

    // chat/forms.ts (the first argument is the time zone the TypeScript ran in)
    case "formAnswer":
        let answer = formAnswer(try a[1].arg([UiField].self), try a[2].map(FieldDraft.self), timeZone: try zone(a[0]))
        return .object([
            "answer": try answer.answer.map(JSONValue.golden) ?? .null,
            "errors": .goldenMap(answer.errors) { .string($0) },
        ])
    case "initialDrafts":
        return try .goldenMap(initialDrafts(try a[1].arg([UiField].self), timeZone: try zone(a[0]))) { try json($0) }
    case "pickChoice":
        return try json(pickChoice(try a[0].arg(UiField.self), try a[1].optional(FieldDraft.self), try a[2].int))
    case "acceptsFormat":
        return .bool(acceptsFormat(try a[1].arg(UiTextFormat.self), try a[2].text, timeZone: try zone(a[0])))
    case "localDateTime": return .string(localDateTime(try a[1].text, timeZone: try zone(a[0])))
    case "fieldAnswer":
        return json(fieldAnswer(try a[1].arg(UiField.self), try a[2].optional(FieldDraft.self), timeZone: try zone(a[0])))

    // chat/queueWait.ts
    case "queueWaitView":
        return try queueWaitView(
            QueueWait(json: a[0]), agent: try a[1].text, context: try a[2].arg(QueueWaitContext.self)
        ).map(JSONValue.golden) ?? .null
    case "composerPlaceholder": return .string(composerPlaceholder(status: try a[0].text, agent: try a[1].text))

    // lib/diff.ts
    case "diffLines": return try .golden(diffLines(try a[0].optional(String.self), try a[1].text))
    case "numberLines": return try .golden(numberLines(try a[0].arg([DiffLine].self)))
    case "changeParts": return try .golden(changeParts(try a[0].arg([NumberedLine].self), context: try a[1].int))
    case "splitLines": return .array(splitLines(try a[0].text).map(JSONValue.string))
    case "diffLinesSummary":
        let result = diffLines(try a[0].text, try a[1].text)
        return .object([
            "added": .number(Double(result.added)),
            "removed": .number(Double(result.removed)),
            "truncated": .bool(result.truncated),
            "count": .number(Double(result.lines.count)),
            "around": try .golden(Array(result.lines[1497..<1503])),
        ])
    case "changePartsSummary": return try .golden(changeParts(try a[0].arg([NumberedLine].self), context: 3))
    case "MAX_DIFF_LINES": return .number(Double(maxDiffLines))

    default:
        throw GoldenError("no port of \(fn)")
    }
}
