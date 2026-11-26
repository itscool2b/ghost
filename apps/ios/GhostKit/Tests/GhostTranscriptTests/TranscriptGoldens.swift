import Foundation
import GhostWire

@testable import GhostTranscript

// Runs a "transcript" golden's steps on the port, as write-goldens.mts runs them on the desktop's
// reducer (`run` there), and gives each check's snapshot in the same shape.

enum GoldenOp: Decodable {
    case empty(cut: String?)
    case apply([Entry])
    case tasks([BackgroundTask])
    case attach(page: HistoryPage, tasks: [BackgroundTask], nextSeq: Int64)
    case frame(EventBatch)
    case earlier(HistoryPage)
    case check(compact: Bool)

    enum CodingKeys: String, CodingKey {
        case op, cut, entries, tasks, page, nextSeq, batch, compact
    }

    init(from decoder: any Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        switch try c.decode(String.self, forKey: .op) {
        case "empty": self = .empty(cut: try c.decodeIfPresent(String.self, forKey: .cut))
        case "apply": self = .apply(try c.decode([Entry].self, forKey: .entries))
        case "tasks": self = .tasks(try c.decode([BackgroundTask].self, forKey: .tasks))
        case "attach":
            self = .attach(
                page: try c.decode(HistoryPage.self, forKey: .page),
                tasks: try c.decode([BackgroundTask].self, forKey: .tasks),
                nextSeq: try c.decode(Int64.self, forKey: .nextSeq))
        case "frame": self = .frame(try c.decode(EventBatch.self, forKey: .batch))
        case "earlier": self = .earlier(try c.decode(HistoryPage.self, forKey: .page))
        case "check": self = .check(compact: try c.decodeIfPresent(Bool.self, forKey: .compact) ?? false)
        case let other: throw GoldenError("unknown op \(other)")
        }
    }
}

struct TranscriptGolden: Decodable {
    struct Input: Decodable { let ops: [GoldenOp] }
    struct Expected: Decodable { let checks: [JSONValue] }
    let name: String
    let input: Input
    let expected: Expected
}

/// The snapshots of a case's checks, run on the port.
func runTranscript(_ ops: [GoldenOp]) throws -> [JSONValue] {
    var model = Transcript.empty()
    /// Every entry since the last "empty", or the web client's `#entries`.
    var kept: [Entry] = []
    var snapshotTasks: [BackgroundTask] = []
    var expected: Int64 = 0
    /// Each seq's event as the case first gave it.
    var originals: [Int64: GhostEvent] = [:]
    func remember(_ entries: [Entry]) {
        for case .event(let entry) in entries where originals[entry.seq] == nil { originals[entry.seq] = entry.event }
    }
    var checks: [JSONValue] = []
    for op in ops {
        switch op {
        case .empty(let cut):
            model = .empty(cut: cut)
            kept = []
        case .apply(let entries):
            remember(entries)
            model = model.applying(entries)
            kept += entries
        case .tasks(let tasks):
            model = withTasks(model, tasks)
        case .attach(let page, let tasks, let nextSeq):
            remember(page.entries)
            kept = compactToolUpdates(page.entries)
            expected = nextSeq
            snapshotTasks = tasks
            model = withTasks(Transcript.empty(cut: page.cutTurnId).applying(kept), snapshotTasks)
        case .frame(let batch):
            remember(batch.entries)
            let fresh = batch.entries.filter { $0.seq >= expected }
            expected = max(expected, batch.seqStart + Int64(batch.events.count))
            kept += fresh
            model = model.applying(fresh)
        case .earlier(let page):
            remember(page.entries)
            kept = compactToolUpdates(page.entries + kept)
            model = withTasks(Transcript.empty(cut: page.cutTurnId).applying(kept), snapshotTasks)
        case .check(let compact):
            checks.append(try snapshot(model, compacted: compact ? compactToolUpdates(kept) : nil, originals: originals))
        }
    }
    return checks
}

private func snapshot(_ model: Transcript, compacted: [Entry]?, originals: [Int64: GhostEvent]) throws -> JSONValue {
    let rendered = renderTranscript(model, live: true)
    let still = renderTranscript(model, live: false)
    let shown = ExactSet(rendered.flatMap { $0.items.map(\.key) })
    var out: [String: JSONValue] = [
        "rendered": try .golden(rendered),
        "still": .array(
            still.map { segment in
                .array([
                    .string(segment.key),
                    .array(
                        segment.items.map { item in
                            .array([.string(item.key), item.display.map { .string($0.rawValue) } ?? item.thinking.map(JSONValue.bool) ?? .null])
                        }),
                ])
            }),
        "segments": .array(
            model.segments.map { .array([.string($0.key), .golden($0.promptId), .array($0.itemKeys.map(JSONValue.string))]) }),
        "order": .array(model.items.keys.map(JSONValue.string)),
        "hidden": try .golden(model.items.orderedValues.filter { !shown.contains($0.key) }),
        "tasks": try .golden(model.tasks.orderedValues),
        "shown": .array(shownTasks(model.tasks).map { .string($0.task.taskId) }),
        "lastErrorKey": .golden(lastErrorKey(model)),
        "hasTurns": .bool(hasTurns(model)),
        "derived": try derived(rendered),
    ]
    if let compacted {
        // The desktop never saw the events this copy does not know: they were left out for it.
        out["compacted"] = .array(
            try compacted.compactMap { entry -> JSONValue? in
                switch entry {
                case .gap(let gap): return .array([.number(Double(gap.seq)), .string("gap")])
                case .event(let row):
                    if case .unknown = row.event { return nil }
                    let same = originals[row.seq] == row.event
                    return .array([.number(Double(row.seq)), same ? .string("same") : try .golden(row.event)])
                }
            })
    }
    return .object(out)
}

/// The helpers that read a rendered turn: its fold, changed files, footer and tool lines.
private func derived(_ rendered: [RenderedSegment]) throws -> JSONValue {
    var tools: [JSONValue] = []
    for segment in rendered {
        for item in segment.items {
            guard case .tool(let tool, let display) = item.content else { continue }
            var words: [String: JSONValue] = [
                "phrase": try .golden(toolPhrase(tool.title, kind: tool.toolKind, display: display))
            ]
            let input = toolInput(tool)
            if !input.isEmpty { words["input"] = try .golden(input) }
            if let delta = diffDelta(tool) { words["delta"] = try .golden(delta) }
            let links = searchLinks(tool)
            if !links.isEmpty { words["links"] = try .golden(links) }
            tools.append(.array([.string(item.key), .object(words)]))
        }
    }
    return .object([
        "folds": .array(try rendered.map { segment in try workFold(segment).map(JSONValue.golden) ?? .null }),
        "changed": .array(rendered.map { .array(changedFiles($0).map(JSONValue.string)) }),
        "labels": .array(rendered.map { .golden($0.end.map(stopReasonLabel)) }),
        "summaries": .array(
            try rendered.map { segment in
                let called = segment.items.compactMap { item -> GroupedTool? in
                    guard case .tool(let tool, let display) = item.content else { return nil }
                    return GroupedTool(toolKind: tool.toolKind, display: display)
                }
                return called.isEmpty ? .null : .array([.string(toolsSummary(called)), try .golden(groupResult(called))])
            }),
        "tools": .array(tools),
    ])
}
