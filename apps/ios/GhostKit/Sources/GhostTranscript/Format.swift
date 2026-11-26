// The number formats of the desktop's `lib/format.ts` that the chat uses: token counts, costs and
// durations.

/// 950 ms, 1.2 s, 12 s, 2 min 5 s. Each step rounds to the precision it shows before choosing the
/// unit, so a value just under a boundary moves up a unit instead of printing "1000 ms", "10.0 s",
/// "60 s" or "1 min 60 s".
public func formatDuration(_ ms: Double) -> String {
    if !ms.isFinite || ms < 0 { return "0 ms" }
    let wholeMs = JS.round(ms)
    if wholeMs < 1000 { return "\(JS.numberString(wholeMs)) ms" }
    let tenths = JS.round(ms / 100)
    if tenths < 100 { return "\(JS.toFixed(tenths / 10, 1)) s" }
    let seconds = JS.round(ms / 1000)
    if seconds < 60 { return "\(JS.numberString(seconds)) s" }
    let minutes = (seconds / 60).rounded(.down)
    return "\(JS.numberString(minutes)) min \(JS.numberString(JS.remainder(seconds, 60))) s"
}

/// 950, 12.5k, 1.2M. Rounds before choosing the unit (as `formatDuration` does), so 999,950 is
/// "1M", not "1000k". Halves round away from zero, the same for negative counts.
public func formatCount(_ count: Double) -> String {
    if !count.isFinite { return "0" }
    let sign = count < 0 ? "-" : ""
    let magnitude = abs(count)
    let whole = JS.round(magnitude)
    if whole < 1000 { return "\(whole == 0 ? "" : sign)\(JS.numberString(whole))" }
    // Tenths of a thousand, then of a million: integers, so the rounding is exact.
    let thousands = JS.round(magnitude / 100)
    if thousands < 10_000 { return "\(sign)\(trimZero(JS.toFixed(thousands / 10, 1)))k" }
    let millions = JS.round(magnitude / 100_000)
    return "\(sign)\(trimZero(JS.toFixed(millions / 10, 1)))M"
}

public func formatCount(_ count: Int64) -> String { formatCount(Double(count)) }

public func formatCount(_ count: Int) -> String { formatCount(Double(count)) }

private func trimZero(_ value: String) -> String {
    value.hasSuffix(".0") ? String(value.dropLast(2)) : value
}

/// $0.0184 or 0.0184 EUR: four decimals, since per-turn costs are cents.
public func formatCost(_ amount: Double?, _ currency: String) -> String? {
    guard let amount, amount.isFinite else { return nil }
    let value = JS.toFixed(amount, 4)
    return currency.same(as: "USD") ? "$\(value)" : "\(value) \(currency)"
}
