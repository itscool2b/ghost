import CryptoKit
import Foundation
import JavaScriptCore

#if canImport(UIKit)
    import UIKit
#endif

/// Code colours from the desktop's own highlight.js (11.12.0, its languages, aliases and limit),
/// run in JavaScriptCore on a serial queue of its own, never the main thread. One JavaScript
/// context is made on first use and let go under memory pressure; results are cached by fence and
/// code.
public actor Highlighter {
    /// Longer code stays plain, as on the desktop: colouring it would cost more than it gives.
    /// Counted in UTF-16 code units, as JavaScript counts.
    public static let maxCharacters = 100_000

    /// The one most of the app shares.
    public static let shared = Highlighter()

    /// How much code the cache holds at most, in UTF-16 code units; the oldest goes first.
    static let cacheBudget = 2_000_000

    private let queue = DispatchSerialQueue(label: "codes.o7.highlight", qos: .userInitiated)
    private var engine: Engine?
    private var cache: [Key: [HighlightRun]] = [:]
    private var order: [(key: Key, cost: Int)] = []
    private var cached = 0

    public nonisolated var unownedExecutor: UnownedSerialExecutor {
        queue.asUnownedSerialExecutor()
    }

    public init() {}

    /// The runs for `code` written in `language` (a fence's name: `swift`, `ts`, `sh`). Empty code
    /// has none. A missing or unknown language, code over `maxCharacters` or anything the strict
    /// parser refuses gives one plain run over the whole code.
    public func tokens(code: String, language: String?) async -> [HighlightRun] {
        let length = code.utf16.count
        guard length > 0 else { return [] }
        let plain = [HighlightRun(range: 0..<length, scopes: [])]
        guard let fence = language,
            !fence.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
            length <= Self.maxCharacters
        else { return plain }
        let key = Key(fence: fence, digest: Self.digest(code))
        if let runs = cache[key] { return runs }
        var runs = plain
        if let html = html(code: code, fence: fence), let parsed = HighlightHTML.parse(html),
            parsed.text.elementsEqual(code.utf16)
        {
            runs = parsed.runs
        }
        remember(runs, for: key, cost: length)
        return runs
    }

    /// The highlight.js language a fence names, after the desktop's aliases (`sh` is `bash`,
    /// `tsx` is `typescript`), or nil when the desktop colours no such language.
    public func language(for fence: String) -> String? {
        guard let engine = loadedEngine(),
            let value = engine.call("languageOf", fence), value.isString
        else { return nil }
        return value.toString()
    }

    /// Lets go of the JavaScript context and the cache. The next call makes them again.
    public func releaseResources() {
        engine = nil
        cache.removeAll()
        order.removeAll()
        cached = 0
    }

    /// The HTML the bundle writes for `code`, or nil when it colours nothing. For tests.
    func html(code: String, fence: String) -> String? {
        guard let engine = loadedEngine(),
            let value = engine.call("highlight", code, fence), value.isString
        else { return nil }
        return value.toString()
    }

    /// The bundle's own version and limit. For tests.
    func bundleConstants() -> (version: String, maxChars: Int)? {
        guard let engine = loadedEngine(),
            let version = engine.api.forProperty("version"), version.isString,
            let limit = engine.api.forProperty("maxChars"), limit.isNumber
        else { return nil }
        return (version.toString(), Int(limit.toInt32()))
    }

    private func loadedEngine() -> Engine? {
        if let engine { return engine }
        guard
            let engine = Engine(queue: queue, onPressure: { [weak self] in
                Task { await self?.releaseResources() }
            })
        else { return nil }
        self.engine = engine
        return engine
    }

    private func remember(_ runs: [HighlightRun], for key: Key, cost: Int) {
        cache[key] = runs
        order.append((key, cost))
        cached += cost
        while cached > Self.cacheBudget, !order.isEmpty {
            let oldest = order.removeFirst()
            cache[oldest.key] = nil
            cached -= oldest.cost
        }
    }

    private static func digest(_ code: String) -> SHA256.Digest {
        var copy = code
        return copy.withUTF8 { SHA256.hash(data: UnsafeRawBufferPointer($0)) }
    }

    private struct Key: Hashable {
        let fence: String
        let digest: SHA256.Digest
    }
}

/// The JavaScript side: a context with the bundle loaded, and the watch that lets it go.
private final class Engine {
    let context: JSContext
    let api: JSValue
    private let watch: MemoryWatch

    /// Where the bundle is: in a Resources folder when the package copies that folder, at the top
    /// when it copies the file alone (which iOS needs: code signing refuses a flat bundle holding a
    /// top-level folder named Resources).
    static let scriptURL: URL? =
        Bundle.module.url(forResource: "highlight", withExtension: "js", subdirectory: "Resources")
        ?? Bundle.module.url(forResource: "highlight", withExtension: "js")

    /// Loads the bundle, or fails when it is missing or does not run.
    init?(queue: DispatchQueue, onPressure: @escaping @Sendable () -> Void) {
        guard let url = Self.scriptURL,
            let script = try? String(contentsOf: url, encoding: .utf8),
            let context = JSContext()
        else { return nil }
        context.name = "o7 highlight"
        // JavaScriptCore has no console; highlight.js only warns through it.
        context.evaluateScript("var console = { log() {}, info() {}, warn() {}, error() {}, debug() {} };")
        context.evaluateScript(script, withSourceURL: url)
        guard context.exception == nil,
            let api = context.objectForKeyedSubscript("O7Highlight"), api.isObject
        else { return nil }
        self.context = context
        self.api = api
        watch = MemoryWatch(queue: queue, onPressure: onPressure)
    }

    /// Calls one of the bundle's functions; nil when it throws or returns nothing.
    func call(_ name: String, _ arguments: Any...) -> JSValue? {
        let value = api.invokeMethod(name, withArguments: arguments)
        if context.exception != nil {
            context.exception = nil
            return nil
        }
        guard let value, !value.isNull, !value.isUndefined else { return nil }
        return value
    }
}

/// Calls `onPressure` when memory runs low: a dispatch memory pressure source on every platform,
/// and UIKit's memory warning on iOS. Both stop when the watch goes.
private final class MemoryWatch: @unchecked Sendable {
    private let source: any DispatchSourceMemoryPressure
    private let observer: (any NSObjectProtocol)?

    init(queue: DispatchQueue, onPressure: @escaping @Sendable () -> Void) {
        source = DispatchSource.makeMemoryPressureSource(eventMask: [.warning, .critical], queue: queue)
        source.setEventHandler(handler: onPressure)
        source.activate()
        #if canImport(UIKit)
            observer = NotificationCenter.default.addObserver(
                forName: UIApplication.didReceiveMemoryWarningNotification, object: nil, queue: nil
            ) { _ in onPressure() }
        #else
            observer = nil
        #endif
    }

    deinit {
        source.cancel()
        if let observer { NotificationCenter.default.removeObserver(observer) }
    }
}
