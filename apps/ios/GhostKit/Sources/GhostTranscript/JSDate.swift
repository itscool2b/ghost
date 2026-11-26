internal import Foundation

// JavaScript's `Date` as `forms.ts` uses it: `new Date(text)`, `toISOString()` and the local-time
// getters, in a time zone given explicitly.
//
// `new Date(text)` reads V8's ES5 ISO format exactly, with V8's own extensions: a fraction of a
// second of any length (as V8 reads its digits), lowercase `t` and `z`, `24:00` for the end of a
// day, `+hhmm` offsets, and days up to 31 in every month (2026-02-30 is March 2). Where V8 falls
// back to its legacy parser (a space instead of `T`, "Nov 5 2026", a year without four digits) this
// gives NaN: a documented divergence. A local time skipped by a clock change reads with the offset
// before the change, and a repeated one as its first occurrence, as V8 does.

enum JSDate {
    static let msPerDay = 86_400_000.0
    private static let maxTime = 8.64e15

    /// `new Date(text).getTime()`, NaN when it is no date.
    static func parse(_ text: String, timeZone: TimeZone) -> Double {
        let units = text.units
        var reader = Reader(units: units)
        guard let parts = reader.es5DateTime() else { return .nan }
        let day = makeDay(year: Double(parts.year), month: Double(parts.month - 1), date: Double(parts.day))
        let time = Double(parts.hour) * 3_600_000 + Double(parts.minute) * 60_000 + Double(parts.second) * 1000
            + Double(parts.millisecond)
        var date = day * msPerDay + time
        if let offset = parts.offsetSeconds {
            date -= Double(offset) * 1000
            if abs(date) > maxTime { return .nan }
        } else {
            if abs(date) > maxTime + 864_000_000 { return .nan }
            date = utc(fromLocal: date, timeZone: timeZone)
        }
        return abs(date) <= maxTime ? date + 0 : .nan
    }

    /// `date.toISOString()` for a valid time.
    static func isoString(_ time: Double) -> String {
        let (year, month, day) = civil(Int64((time / msPerDay).rounded(.down)))
        let ms = Int64(time - (time / msPerDay).rounded(.down) * msPerDay)
        let yearText: String
        if year >= 0 && year <= 9999 {
            yearText = pad(year, 4)
        } else {
            yearText = (year < 0 ? "-" : "+") + pad(abs(year), 6)
        }
        return "\(yearText)-\(pad(month, 2))-\(pad(day, 2))T\(pad(ms / 3_600_000, 2)):"
            + "\(pad(ms / 60_000 % 60, 2)):\(pad(ms / 1000 % 60, 2)).\(pad(ms % 1000, 3))Z"
    }

    /// The local year, month (1 to 12), day, hour and minute of a valid time, as the `get…`
    /// methods give them.
    static func localParts(_ time: Double, timeZone: TimeZone) -> (Int64, Int64, Int64, Int64, Int64) {
        let local = time + offset(at: time, timeZone)
        let days = (local / msPerDay).rounded(.down)
        let (year, month, day) = civil(Int64(days))
        let ms = Int64(local - days * msPerDay)
        return (year, month, day, ms / 3_600_000, ms / 60_000 % 60)
    }

    private static func pad(_ value: Int64, _ width: Int) -> String {
        let digits = String(value)
        return digits.count >= width ? digits : String(repeating: "0", count: width - digits.count) + digits
    }

    // MARK: Calendar

    /// ECMAScript's MakeDay: days since 1970 of a year, a month from 0 (any value: it rolls over)
    /// and a day of the month (any value).
    static func makeDay(year: Double, month: Double, date: Double) -> Double {
        let ym = year + (month / 12).rounded(.down)
        let mn = month - (month / 12).rounded(.down) * 12
        return Double(daysFromCivil(Int64(ym), Int64(mn) + 1, 1)) + date - 1
    }

    /// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
    private static func daysFromCivil(_ year: Int64, _ month: Int64, _ day: Int64) -> Int64 {
        let y = month <= 2 ? year - 1 : year
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = (153 * (month > 2 ? month - 3 : month + 9) + 2) / 5 + day - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        return era * 146_097 + doe - 719_468
    }

    /// The proleptic Gregorian date of a day since 1970-01-01.
    private static func civil(_ days: Int64) -> (Int64, Int64, Int64) {
        let z = days + 719_468
        let era = (z >= 0 ? z : z - 146_096) / 146_097
        let doe = z - era * 146_097
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100)
        let mp = (5 * doy + 2) / 153
        let day = doy - (153 * mp + 2) / 5 + 1
        let month = mp < 10 ? mp + 3 : mp - 9
        return (month <= 2 ? yoe + era * 400 + 1 : yoe + era * 400, month, day)
    }

    // MARK: Time zones

    /// The zone's offset from UTC at an instant, in milliseconds.
    static func offset(at time: Double, _ timeZone: TimeZone) -> Double {
        Double(timeZone.secondsFromGMT(for: Date(timeIntervalSince1970: time / 1000))) * 1000
    }

    /// The instant a local time names: the offset in effect before a clock change, for a time the
    /// change repeats or skips.
    private static func utc(fromLocal local: Double, timeZone: TimeZone) -> Double {
        let before = offset(at: local - msPerDay, timeZone)
        let after = offset(at: local + msPerDay, timeZone)
        let candidates = [local - before, local - after].filter { offset(at: $0, timeZone) == local - $0 }
        return candidates.min() ?? local - before
    }

    // MARK: V8's ES5 date-time reader

    struct Parts {
        var year = 0
        var month = 1
        var day = 1
        var hour = 0
        var minute = 0
        var second = 0
        var millisecond = 0
        var offsetSeconds: Int?
    }

    /// V8's tokens for digits: their value (the first nine digits after any leading zeros, as V8
    /// reads them) and how many digits there were.
    struct Number {
        let value: Int
        let length: Int
    }

    struct Reader {
        let units: Units
        var at = 0

        var end: Bool { at >= units.count }

        func peekDigits() -> Number? {
            guard at < units.count, JS.isDigit(units[at]) else { return nil }
            var index = at
            while index < units.count && units[index] == 0x30 { index += 1 }
            var value = 0
            var significant = 0
            while index < units.count && JS.isDigit(units[index]) {
                if significant < 9 { value = value * 10 + Int(units[index] - 0x30) }
                significant += 1
                index += 1
            }
            return Number(value: value, length: index - at)
        }

        mutating func digits(length: Int) -> Int? {
            guard let number = peekDigits(), number.length == length else { return nil }
            at += length
            return number.value
        }

        mutating func skip(_ unit: UInt16) -> Bool {
            guard at < units.count, units[at] == unit else { return false }
            at += 1
            return true
        }

        /// `[('-'|'+')yyyyyy | yyyy][-MM[-DD]][THH:mm[:ss[.s+]][Z|(+|-)hh:mm|(+|-)hhmm]]`. Nil both
        /// where V8 gives up and where it would read on with its legacy parser.
        mutating func es5DateTime() -> Parts? {
            var parts = Parts()
            if at < units.count && (units[at] == 0x2B || units[at] == 0x2D) {
                let negative = units[at] == 0x2D
                at += 1
                guard let year = digits(length: 6), !(negative && year == 0) else { return nil }
                parts.year = negative ? -year : year
            } else {
                guard let year = digits(length: 4) else { return nil }
                parts.year = year
            }
            if skip(0x2D) {
                guard let month = digits(length: 2), (1...12).contains(month) else { return nil }
                parts.month = month
                if skip(0x2D) {
                    guard let day = digits(length: 2), (1...31).contains(day) else { return nil }
                    parts.day = day
                }
            }
            if end {
                // A date-only form is UTC.
                parts.offsetSeconds = 0
                return parts
            }
            // The time separator is a word of its own: `T` or `t` with no letter after it.
            guard units[at] == 0x54 || units[at] == 0x74 else { return nil }
            if at + 1 < units.count && units[at + 1] >= 0x41 { return nil }
            at += 1
            guard let hour = digits(length: 2), hour <= 24 else { return nil }
            let endOfDay = hour == 24
            guard skip(0x3A), let minute = digits(length: 2), minute <= 59, !(endOfDay && minute > 0) else {
                return nil
            }
            parts.hour = hour
            parts.minute = minute
            if skip(0x3A) {
                guard let second = digits(length: 2), second <= 59, !(endOfDay && second > 0) else { return nil }
                parts.second = second
                if skip(0x2E) {
                    guard let fraction = peekDigits(), !(endOfDay && fraction.value > 0) else { return nil }
                    at += fraction.length
                    parts.millisecond = Self.milliseconds(fraction)
                }
            }
            if at < units.count && (units[at] == 0x5A || units[at] == 0x7A)
                && !(at + 1 < units.count && units[at + 1] >= 0x41)
            {
                at += 1
                parts.offsetSeconds = 0
            } else if at < units.count && (units[at] == 0x2B || units[at] == 0x2D) {
                let sign = units[at] == 0x2B ? 1 : -1
                at += 1
                let hours: Int
                let minutes: Int
                if let both = peekDigits(), both.length == 4 {
                    at += 4
                    hours = both.value / 100
                    minutes = both.value % 100
                    guard hours <= 23, minutes <= 59 else { return nil }
                } else {
                    guard let h = digits(length: 2), h <= 23, skip(0x3A), let m = digits(length: 2), m <= 59 else {
                        return nil
                    }
                    hours = h
                    minutes = m
                }
                parts.offsetSeconds = sign * (hours * 3600 + minutes * 60)
            }
            return end ? parts : nil
        }

        /// V8's ReadMilliseconds: the first three digits of the fraction, from the value it read
        /// and the digit count (capped at nine), leading zeros and all.
        static func milliseconds(_ number: Number) -> Int {
            var value = number.value
            var length = number.length
            if length == 1 { return value * 100 }
            if length == 2 { return value * 10 }
            if length > 9 { length = 9 }
            while length > 3 {
                value /= 10
                length -= 1
            }
            return value
        }
    }
}
