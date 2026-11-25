public import O7Wire

// Background tasks (`tasks.ts`): the subagents and background commands an agent starts. The
// reducer keeps each task's record and steps (`task_updated`, `task_step`) beside its items, so a
// history page and the live stream give the same tasks; the dock's strip and the task panel read
// them from there.

/// One task as the chat knows it.
public struct TaskItem: Sendable, Hashable, Codable {
    public var task: BackgroundTask
    /// Its steps in the order they first arrived, each replaced by its id.
    public var steps: [TaskStep]
    /// The newest turn's key when the task reported its end, so the strip keeps showing it until
    /// the next turn starts; nil while it runs.
    public var endedIn: String?
    /// The seq it was first heard of at, for a stable order.
    public var order: Int64
    /// Only steps arrived so far: its record is in an older history page. Never shown.
    public var placeholder: Bool

    public init(
        task: BackgroundTask, steps: [TaskStep], endedIn: String?, order: Int64, placeholder: Bool
    ) {
        self.task = task
        self.steps = steps
        self.endedIn = endedIn
        self.order = order
        self.placeholder = placeholder
    }
}

/// A step for a task whose record is not loaded yet: kept, but not shown until the record comes.
private func placeholderTask(_ taskId: String) -> BackgroundTask {
    BackgroundTask(taskId: taskId, kind: .agent, title: "Subagent", backgrounded: false, status: .running)
}

/// A record over what is known: nil prompt and summary keep the ones already known.
public func mergeTask(_ previous: TaskItem?, _ task: BackgroundTask, order: Int64, turnKey: String?)
    -> TaskItem
{
    let known = previous?.placeholder == false ? previous?.task : nil
    // Where it came from never changes: a record that leaves it out keeps what is known.
    var merged = task
    merged.agentType = task.agentType ?? known?.agentType
    merged.toolCallId = task.toolCallId ?? known?.toolCallId
    merged.parentTaskId = task.parentTaskId ?? known?.parentTaskId
    merged.startedAt = task.startedAt ?? known?.startedAt
    merged.prompt = task.prompt ?? known?.prompt
    merged.summary = task.summary ?? known?.summary
    merged.summaryTruncated = task.summary != nil ? task.summaryTruncated : known?.summaryTruncated
    let ended = merged.status != .running
    return TaskItem(
        task: merged,
        steps: previous?.steps ?? [],
        endedIn: ended ? (previous?.endedIn ?? turnKey) : nil,
        order: previous?.order ?? order,
        placeholder: false)
}

/// Adds the step, or replaces the one with its id in place.
public func withStep(_ previous: TaskItem?, taskId: String, step: TaskStep, order: Int64) -> TaskItem {
    var item =
        previous
        ?? TaskItem(task: placeholderTask(taskId), steps: [], endedIn: nil, order: order, placeholder: true)
    if let index = item.steps.firstIndex(where: { $0.stepId.same(as: step.stepId) }) {
        item.steps[index] = step
    } else {
        item.steps.append(step)
    }
    return item
}

/// What a task's mark shows: its own status, or stopped when it says it runs but nothing will end
/// it: the session has no process, or the live session does not know it (it ran in an earlier one).
public func taskDisplay(_ item: TaskItem, live: Bool, liveTasks: Set<String>) -> TaskStatus {
    if item.task.status != .running { return item.task.status }
    let known = liveTasks.contains { $0.same(as: item.task.taskId) }
    return live && known ? .running : .stopped
}

/// How long the task has run (still running: until `now`), or nil when nothing says.
public func taskElapsed(_ task: BackgroundTask, display: TaskStatus, now: Double) -> Double? {
    if display == .running, let startedAt = task.startedAt {
        return JS.max(now - Double(startedAt), Double(task.durationMs ?? 0))
    }
    if let durationMs = task.durationMs { return Double(durationMs) }
    if let startedAt = task.startedAt, let endedAt = task.endedAt {
        return JS.max(0, Double(endedAt) - Double(startedAt))
    }
    return nil
}

public func elapsedLabel(_ ms: Double?) -> String? {
    ms.map(formatElapsed)
}

/// The strip's short form: "42s", "1m 13s", "2h 5m".
public func compactElapsed(_ ms: Double?) -> String? {
    guard let ms else { return nil }
    let seconds = ms.isFinite && ms > 0 ? (ms / 1000).rounded(.down) : 0
    if seconds < 60 { return "\(JS.numberString(seconds))s" }
    let minutes = (seconds / 60).rounded(.down)
    if minutes < 60 { return "\(JS.numberString(minutes))m \(JS.numberString(JS.remainder(seconds, 60)))s" }
    return "\(JS.numberString((minutes / 60).rounded(.down)))h \(JS.numberString(JS.remainder(minutes, 60)))m"
}

/// "12 tools", "1 tool".
public func toolUsesLabel(_ count: Int?) -> String? {
    guard let count else { return nil }
    return "\(formatCount(count)) \(count == 1 ? "tool" : "tools")"
}

/// "48.2k tokens".
public func taskTokensLabel(_ count: Int64?) -> String? {
    count.map { "\(formatCount($0)) tokens" }
}

public func taskStatusLabel(_ status: TaskStatus) -> String {
    switch status {
    case .running: "Running"
    case .completed: "Done"
    case .failed: "Failed"
    case .stopped: "Stopped"
    }
}

/// The key of the newest turn segment, which a task that ends now ends in.
public func newestTurnKey(_ segments: [Segment]) -> String? {
    segments.last { $0.turnId != nil }?.key
}

/// Every shown task, oldest first.
public func shownTasks(_ tasks: OrderedMap<TaskItem>) -> [TaskItem] {
    // A stable sort, as JavaScript's is.
    tasks.orderedValues.filter { !$0.placeholder }.enumerated()
        .sorted { $0.element.order != $1.element.order ? $0.element.order < $1.element.order : $0.offset < $1.offset }
        .map(\.element)
}

/// What the dock's strip lists: the tasks no other task started that run now, and those that ended
/// in the newest turn. A task left running by an earlier process is not listed.
public func stripTasks(
    _ tasks: OrderedMap<TaskItem>, turnKey: String?, live: Bool, liveTasks: Set<String>
) -> [TaskItem] {
    shownTasks(tasks).filter { item in
        if let parent = item.task.parentTaskId, tasks.contains(parent) { return false }
        if taskDisplay(item, live: live, liveTasks: liveTasks) == .running { return true }
        guard item.task.status != .running, let endedIn = item.endedIn, let turnKey else { return false }
        return endedIn.same(as: turnKey)
    }
}

/// The tasks `parentId` started, oldest first.
public func childTasks(_ tasks: OrderedMap<TaskItem>, parentId: String) -> [TaskItem] {
    shownTasks(tasks).filter { $0.task.parentTaskId?.same(as: parentId) == true }
}

/// The task each transcript tool call started, by the call's id.
public func tasksByToolCall(_ tasks: OrderedMap<TaskItem>) -> OrderedMap<String> {
    var byCall = OrderedMap<String>()
    for (_, item) in tasks where !item.placeholder {
        if let call = item.task.toolCallId { byCall[call] = item.task.taskId }
    }
    return byCall
}

/// The words a screen reader hears once when a task ends: "Map account switching code finished".
public func endedAnnouncement(_ task: BackgroundTask, display: TaskStatus) -> String? {
    switch display {
    case .completed: "\(task.title) finished"
    case .failed: "\(task.title) failed"
    case .stopped: "\(task.title) stopped"
    case .running: nil
    }
}

/// The task's last text step, when it repeats the summary: the summary shows it once, in full.
public func repeatsSummary(_ step: TaskStep?, summary: String?) -> Bool {
    guard let step, step.kind == .text, let summary, let stepText = step.text else { return false }
    let text = JS.trim(stepText.units)
    let report = JS.trim(summary.units)
    return !text.isEmpty && (JS.startsWith(report, text) || JS.startsWith(text, report))
}
