import O7Marks
import O7Push
import O7Theme
import SwiftUI
import WidgetKit

/// What needs you and what works, on every paired computer, from what the app and the
/// notification extension last wrote to the App Group. Never a chat's words beyond its title.
struct NeedsYouWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "o7.needs-you", provider: SnapshotProvider()) { entry in
            NeedsYouView(entry: entry)
                .containerBackground(for: .widget) { WidgetLook.background(entry.snapshot) }
        }
        .configurationDisplayName("Needs you")
        .description("What waits on you and what works on your computers.")
        .supportedFamilies([
            .systemSmall, .systemMedium, .accessoryCircular, .accessoryRectangular, .accessoryInline,
        ])
    }
}

struct SnapshotEntry: TimelineEntry {
    let date: Date
    let snapshot: SharedSnapshot
}

struct SnapshotProvider: TimelineProvider {
    func placeholder(in context: Context) -> SnapshotEntry {
        SnapshotEntry(date: .now, snapshot: .example)
    }

    func getSnapshot(in context: Context, completion: @escaping (SnapshotEntry) -> Void) {
        completion(SnapshotEntry(date: .now, snapshot: context.isPreview ? .example : SharedSnapshot.load()))
    }

    func getTimeline(in context: Context, completion: @escaping (Timeline<SnapshotEntry>) -> Void) {
        // The app and the notification extension ask for a new timeline when the counts change;
        // this one only keeps the times fresh.
        let entry = SnapshotEntry(date: .now, snapshot: SharedSnapshot.load())
        completion(Timeline(entries: [entry], policy: .after(.now.addingTimeInterval(30 * 60))))
    }
}

enum WidgetLook {
    static func palette(_ snapshot: SharedSnapshot) -> Palette {
        let first = snapshot.computers.first
        let theme = first?.theme.flatMap(ThemeID.init(rawValue:)) ?? .default
        return Palettes.palette(theme, dark: first?.dark ?? true)
    }

    static func background(_ snapshot: SharedSnapshot) -> Color {
        Color(palette(snapshot).window)
    }
}

struct NeedsYouView: View {
    @Environment(\.widgetFamily) private var family
    let entry: SnapshotEntry

    var body: some View {
        let snapshot = entry.snapshot
        let palette = WidgetLook.palette(snapshot)
        switch family {
        case .accessoryInline:
            Text(inline(snapshot))
        case .accessoryCircular:
            ZStack {
                AccessoryWidgetBackground()
                VStack(spacing: 0) {
                    Text("\(snapshot.asking)").font(.title2.weight(.semibold)).monospacedDigit()
                    Text("needs you").font(.system(size: 8))
                }
            }
            .widgetURL(URL(string: "o7://inbox"))
        case .accessoryRectangular:
            VStack(alignment: .leading, spacing: 2) {
                Text("o7").font(.headline)
                Text(counts(snapshot)).font(.caption)
                if let chat = snapshot.computers.flatMap(\.chats).first {
                    Text(chat.title).font(.caption).lineLimit(1)
                }
            }
            .widgetURL(URL(string: "o7://inbox"))
        case .systemMedium:
            VStack(alignment: .leading, spacing: 8) {
                HStack {
                    header(snapshot, palette)
                    Spacer(minLength: 8)
                    Text(counts(snapshot))
                        .font(.caption)
                        .monospacedDigit()
                        .foregroundStyle(snapshot.asking > 0 ? Color(palette.accent) : Color(palette.fg3))
                        .lineLimit(1)
                }
                let chats = Array(snapshot.computers.flatMap { computer in
                    computer.chats.map { (computer, $0) }
                }.prefix(3))
                if chats.isEmpty {
                    Text("Nothing waits on you.")
                        .font(.footnote)
                        .foregroundStyle(Color(palette.fg3))
                } else {
                    ForEach(chats, id: \.1.id) { computer, chat in
                        Link(destination: URL(string: "o7://chat/\(computer.hostId)/\(chat.id)")!) {
                            ChatLine(chat: chat, palette: palette)
                        }
                    }
                }
                Spacer(minLength: 0)
            }
            .environment(\.o7Palette, palette)
        default:
            VStack(alignment: .leading, spacing: 6) {
                header(snapshot, palette)
                Spacer(minLength: 0)
                Text("\(snapshot.asking)")
                    .font(.system(size: 40, weight: .semibold))
                    .monospacedDigit()
                    .foregroundStyle(snapshot.asking > 0 ? Color(palette.accent) : Color(palette.fg1))
                Text(snapshot.asking == 1 ? "needs you" : "need you")
                    .font(.footnote)
                    .foregroundStyle(Color(palette.fg2))
                Text("\(snapshot.working) working")
                    .font(.caption)
                    .foregroundStyle(Color(palette.fg3))
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .environment(\.o7Palette, palette)
            .widgetURL(URL(string: "o7://inbox"))
        }
    }

    private func header(_ snapshot: SharedSnapshot, _ palette: Palette) -> some View {
        HStack(spacing: 6) {
            O7Logo().fill(Color(palette.fg1)).frame(width: 18, height: 12)
            Text(snapshot.computers.count == 1 ? snapshot.computers[0].hostName : "o7")
                .font(.caption.weight(.semibold))
                .foregroundStyle(Color(palette.fg2))
                .lineLimit(1)
        }
    }

    private func counts(_ snapshot: SharedSnapshot) -> String {
        "\(snapshot.asking) need\(snapshot.asking == 1 ? "s" : "") you · \(snapshot.working) working"
    }

    private func inline(_ snapshot: SharedSnapshot) -> String {
        snapshot.asking > 0 ? "o7: \(snapshot.asking) need\(snapshot.asking == 1 ? "s" : "") you" : "o7: \(snapshot.working) working"
    }
}

private struct ChatLine: View {
    let chat: SharedSnapshot.Chat
    let palette: Palette

    var body: some View {
        HStack(spacing: 8) {
            AgentMark(agent: chat.agent, state: state, size: 18, animated: false)
            Text(chat.title)
                .font(.footnote.weight(.medium))
                .foregroundStyle(Color(palette.fg1))
                .lineLimit(1)
            Spacer(minLength: 0)
        }
    }

    private var state: AgentState {
        switch chat.state {
        case "asking": .waiting
        case "working": .tool
        case "failed": .error
        case "done": .done
        default: .idle
        }
    }
}

extension SharedSnapshot {
    static let example = SharedSnapshot(computers: [
        .init(
            hostId: "example", hostName: "Studio", asking: 1, working: 2,
            chats: [
                .init(id: "c1", title: "Tidy the changelog", agent: "claude-code", state: "asking", updatedAt: 0),
                .init(id: "c2", title: "Ship the iPhone app", agent: "codex", state: "working", updatedAt: 0),
            ], theme: "claude", dark: true)
    ])
}
