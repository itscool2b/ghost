import O7Marks
import O7Transcript
import O7Wire
import SwiftUI

/// Background tasks (subagents, commands left running) as glass chips over the message box; a tap
/// opens one's steps.
struct TaskStrip: View {
    @Environment(\.style) private var style
    let tasks: [TaskItem]
    let live: Bool
    let liveTasks: Set<String>
    @State private var shown: TaskItem?

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            GlassEffectContainer(spacing: 8) {
                HStack(spacing: 8) {
                    ForEach(tasks, id: \.task.taskId) { item in
                        let status = taskDisplay(item, live: live, liveTasks: liveTasks)
                        Button {
                            shown = item
                        } label: {
                            HStack(spacing: 8) {
                                if status == .running {
                                    PixelLoader(size: 12, tempo: .tool)
                                } else {
                                    Image(systemName: status == .completed ? "checkmark" : status == .failed ? "xmark" : "stop.fill")
                                        .font(.caption2.weight(.bold))
                                        .foregroundStyle(status == .failed ? style.red : .secondary)
                                }
                                Text(item.task.title)
                                    .o7Font(.ui(13, weight: .medium))
                                    .lineLimit(1)
                                if let activity = item.task.activity, status == .running {
                                    Text(activity).o7Font(.ui(12)).foregroundStyle(.secondary).lineLimit(1)
                                }
                            }
                            .padding(.horizontal, 12)
                            .padding(.vertical, 7)
                        }
                        .buttonStyle(.plain)
                        .glassEffect(.regular.interactive(), in: .capsule)
                    }
                }
                .padding(.horizontal, 12)
            }
        }
        .sheet(item: $shown) { item in
            TaskSheet(item: item, live: live, liveTasks: liveTasks)
                .presentationDetents([.medium, .large])
        }
    }
}

extension TaskItem: @retroactive Identifiable {
    public var id: String { task.taskId }
}

/// One background task: what it was asked, what it did, how it ended.
struct TaskSheet: View {
    @Environment(\.style) private var style
    @Environment(\.dismiss) private var dismiss
    let item: TaskItem
    let live: Bool
    let liveTasks: Set<String>

    var body: some View {
        let status = taskDisplay(item, live: live, liveTasks: liveTasks)
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    HStack(spacing: 8) {
                        Text(taskStatusLabel(status)).o7Font(.meta).foregroundStyle(status == .failed ? style.red : style.fg3)
                        if let elapsed = elapsedLabel(taskElapsed(item.task, display: status, now: Date().timeIntervalSince1970 * 1000)) {
                            Text(elapsed).o7Font(.meta).monospacedDigit().foregroundStyle(style.fg3)
                        }
                        if let uses = toolUsesLabel(item.task.toolUses) {
                            Text(uses).o7Font(.meta).foregroundStyle(style.fg3)
                        }
                    }
                    if let prompt = item.task.prompt, !prompt.isEmpty {
                        Text(prompt).o7Font(.chat).foregroundStyle(style.fg2).textSelection(.enabled)
                    }
                    ForEach(Array(item.steps.enumerated()), id: \.offset) { _, step in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Image(systemName: step.kind == .tool ? icon(step.toolKind) : "text.alignleft")
                                .font(.caption)
                                .foregroundStyle(style.fg3)
                                .frame(width: 16)
                            VStack(alignment: .leading, spacing: 2) {
                                Text(step.title).o7Font(.chat).foregroundStyle(style.fg1)
                                if let text = step.text, !text.isEmpty {
                                    Text(text).o7Font(.small).foregroundStyle(style.fg3).lineLimit(6)
                                }
                            }
                        }
                    }
                    if let summary = item.task.summary, !summary.isEmpty {
                        MarkdownView(summary)
                    }
                }
                .padding(20)
            }
            .background(style.raised.ignoresSafeArea())
            .navigationTitle(item.task.title)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done", systemImage: "checkmark") { dismiss() }
                }
            }
        }
    }
}
