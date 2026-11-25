import Foundation
import O7Wire
import Testing

@testable import O7Transcript

// What the goldens cannot see: revisions and render reuse (which the desktop gets from object
// identity), purity of the value types, and the JavaScript semantics the port leans on.

private func entries(_ events: [(String?, O7Event)], from first: Int64 = 0) -> [Entry] {
    events.enumerated().map { .event(seq: first + Int64($0.offset), turnId: $0.element.0, event: $0.element.1) }
}

private let start = O7Event.turnStarted(turnId: "t1", promptId: nil)
private let read = O7Event.toolCall(ToolCall(toolCallId: "a", kind: .read, title: "Read a", status: .inProgress))

@Suite("Revisions and render reuse")
struct RevisionTests {
    @Test func reusesATurnWhileNothingShownChanges() {
        var cache = RenderCache()
        var model = Transcript.empty().applying(entries([(nil, start), (nil, read)]))
        let before = cache.render(model, live: true)[0]
        #expect(cache.render(model, live: true)[0].revision == before.revision)
        model = reduce(model, seq: 3, turnId: nil, event: .thoughtDelta(thoughtId: "th", text: "Hmm"))
        let thinking = cache.render(model, live: true)[0]
        #expect(thinking.revision != before.revision)
        model = reduce(model, seq: 4, turnId: nil, event: .thoughtDelta(thoughtId: "th", text: ", then"))
        let next = cache.render(model, live: true)[0]
        #expect(next.revision != thinking.revision)
        #expect(next.items.last?.thinking == true)
        // The same model not live renders apart from live.
        #expect(cache.render(model, live: false)[0].revision != next.revision)
    }

    @Test func revisionsChangeOnlyWhereTheReducerTouched() {
        let model = Transcript.empty().applying(
            entries([
                (nil, start), (nil, .messageDelta(messageId: "m", text: "a")),
                (nil, .turnEnded(turnId: "t1", stopReason: .endTurn, usage: nil, durationMs: nil, recovered: false)),
                (nil, .notice(level: .info, message: "between")),
            ]))
        let first = renderTranscript(model, live: true)
        let later = model.applying(entries([(nil, .notice(level: .info, message: "more"))], from: 9))
        let second = renderTranscript(later, live: true)
        #expect(second[0].revision == first[0].revision)
        #expect(second[1].revision != first[1].revision)
        #expect(second[1].items[0].revision == first[1].items[0].revision)
        // An update to an earlier turn's message gives that turn a new revision.
        let updated = later.applying(entries([(nil, .messageDelta(messageId: "m", text: "b"))], from: 10))
        #expect(renderTranscript(updated, live: true)[0].revision != second[0].revision)
    }

    @Test func cachedRenderingEqualsAFreshOne() {
        var cache = RenderCache()
        var model = Transcript.empty()
        let events: [O7Event] = [
            start, read, .thoughtDelta(thoughtId: "th", text: ""),
            .toolUpdate(ToolUpdate(toolCallId: "a", status: .completed)),
            .thoughtCompleted(thoughtId: "th", durationMs: 0),
            .turnEnded(turnId: "t1", stopReason: .cancelled, usage: nil, durationMs: 5, recovered: false),
        ]
        for (index, event) in events.enumerated() {
            model = reduce(model, seq: Int64(index), turnId: nil, event: event)
            for live in [true, false] {
                #expect(cache.render(model, live: live) == renderTranscript(model, live: live))
            }
        }
    }
}

@Suite("Value semantics")
struct PurityTests {
    @Test func theModelGivenStaysAsItWas() {
        let before = Transcript.empty().applying(entries([(nil, start), (nil, .messageDelta(messageId: "m", text: "a"))]))
        let after = reduce(before, seq: 2, turnId: nil, event: .messageDelta(messageId: "m", text: "b"))
        guard case .message(let kept) = before.items["message:m"], case .message(let grown) = after.items["message:m"] else {
            Issue.record("no message")
            return
        }
        #expect(kept.text == "a")
        #expect(grown.text == "ab")
        #expect(before.segments[0].itemKeys == ["message:m"])
        #expect(after.applying([]) == after)
    }

    @Test func compactedEntriesBuildTheSameTranscript() {
        let content = [ToolContent.text(text: "1", truncated: nil)]
        let raw = entries([
            (nil, start),
            (nil, .toolUpdate(ToolUpdate(toolCallId: "early", title: "Early", content: content))),
            (nil, .toolCall(ToolCall(toolCallId: "run", kind: .execute, title: "Run tests", status: .pending))),
            (nil, .toolUpdate(ToolUpdate(toolCallId: "run", content: content))),
            (nil, .toolUpdate(ToolUpdate(toolCallId: "early", status: .completed))),
            (nil, .toolUpdate(ToolUpdate(toolCallId: "run", title: "Run all tests"))),
            (nil, .toolCall(ToolCall(toolCallId: "run", kind: .execute, title: "Again", status: .pending))),
            (nil, .toolUpdate(ToolUpdate(toolCallId: "run", status: .failed))),
        ])
        let folded = compactToolUpdates(raw)
        #expect(folded.count == raw.count - 2)
        #expect(Transcript.empty().applying(folded) == Transcript.empty().applying(raw))
    }

    @Test func unknownEventsAreSkipped() throws {
        let event = try JSONDecoder().decode(O7Event.self, from: Data(#"{"type":"from_the_future","x":1}"#.utf8))
        let model = Transcript.empty().applying(entries([(nil, start), (nil, event)]))
        #expect(model.items.isEmpty)
        #expect(model.segments.count == 1)
    }
}

@Suite("JavaScript semantics")
struct JavaScriptTests {
    @Test func searchesAndSlicesByCodeUnit() {
        let text = "a\r\nb e\u{301} 😀".units
        #expect(JS.indexOf(text, "\n".units) == 2)
        #expect(String(units: JS.slice(text, 0, 2)) == "a\r")
        #expect(JS.split(text, [0x0A]).count == 2)
        #expect(!"e\u{301}".same(as: "\u{e9}"))
        #expect(OrderedMap([("e\u{301}", 1), ("\u{e9}", 2)]).count == 2)
    }

    @Test func trimsECMAScriptWhitespaceOnly() {
        #expect(String(units: JS.trim(" \u{feff}\u{a0}x\u{2028}".units)) == "x")
        #expect(String(units: JS.trim("\u{85}x".units)) == "\u{85}x")
        #expect(String(units: JS.trim("\u{200b}".units)) == "\u{200b}")
    }

    @Test func roundsAndFormatsNumbersAsJavaScriptDoes() {
        #expect(JS.round(2.5) == 3)
        #expect(JS.round(-2.5) == -2)
        #expect(JS.round(0.49999999999999994) == 0)
        #expect(JS.toFixed(2.5, 0) == "3")
        #expect(JS.toFixed(1.005, 2) == "1.00")
        #expect(JS.toFixed(0.25, 1) == "0.3")
        #expect(JS.toFixed(-0.00001, 4) == "-0.0000")
        #expect(JS.toFixed(1e21, 2) == "1e+21")
        #expect(JS.numberString(1e21) == "1e+21")
        #expect(JS.numberString(123e18) == "123000000000000000000")
        #expect(JS.numberString(1e-7) == "1e-7")
        #expect(JS.numberString(0.000001) == "0.000001")
        #expect(JS.numberString(-0.0) == "0")
        #expect(JS.toNumber(" 0x1F ") == 31)
        #expect(JS.toNumber("5.") == 5)
        #expect(JS.toNumber(".5") == 0.5)
        #expect(JS.toNumber("1_0").isNaN)
        #expect(JS.toNumber("nan").isNaN)
        #expect(JS.toNumber("").isZero)
    }

    @Test func lowercasesWithFinalSigma() {
        #expect(JS.lowercased("ΑΣ ΑΣ.") == "ας ας.")
        #expect(JS.lowercased("Σ") == "σ")
        #expect(JS.capitalized("ßx") == "SSx")
        #expect(JS.capitalized("𐐨x") == "𐐨x")
    }

    @Test func readsAndWritesJSONAsJavaScriptDoes() throws {
        let parsed = try #require(JSValue.parse(#"{"b":1,"a":2,"b":3,"10":4,"2":5,"x":"\ud800"}"#.units))
        #expect(String(units: parsed.stringified()) == #"{"2":5,"10":4,"b":3,"a":2,"x":"\ud800"}"#)
        #expect(JSValue.parse("[1,]".units) == nil)
        #expect(JSValue.parse("01".units) == nil)
    }
}
