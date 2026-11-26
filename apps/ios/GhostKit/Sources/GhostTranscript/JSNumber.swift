internal import Foundation

// JavaScript's number semantics: rounding, `toFixed`, `String(number)` and `Number(string)`.
// Swift's own differ: `rounded()` and printf round halves to even, `description` writes "1e-05"
// and "100.0", and `Double(String)` reads "nan", "inf" and hexadecimal floats.

extension JS {
    /// `Math.round`: halves go up, towards positive infinity, and the result keeps the sign of
    /// zero (`Math.round(-0.4)` is -0).
    static func round(_ value: Double) -> Double {
        guard value.isFinite, value != 0 else { return value }
        if value > 0 && value < 0.5 { return 0 }
        if value < 0 && value >= -0.5 { return -0.0 }
        let floor = value.rounded(.down)
        return value - floor >= 0.5 ? floor + 1 : floor
    }

    /// `Math.max(a, b)`: NaN wins.
    static func max(_ a: Double, _ b: Double) -> Double {
        if a.isNaN || b.isNaN { return .nan }
        if a == 0 && b == 0 { return a.sign == .minus ? b : a }
        return a > b ? a : b
    }

    /// `Number.isInteger`.
    static func isInteger(_ value: Double) -> Bool {
        value.isFinite && value.rounded(.towardZero) == value
    }

    /// `x % y`, which keeps the sign of `x`.
    static func remainder(_ x: Double, _ y: Double) -> Double {
        x.truncatingRemainder(dividingBy: y)
    }

    // MARK: toFixed

    /// `value.toFixed(digits)`: the exact binary value rounded to `digits` decimals, a half going
    /// to the larger magnitude. Values from 1e21 up are written as `String(value)`.
    static func toFixed(_ value: Double, _ digits: Int) -> String {
        if value.isNaN { return "NaN" }
        if Swift.abs(value) >= 1e21 { return numberString(value) }
        let sign = value < 0 ? "-" : ""
        let magnitude = Swift.abs(value)
        // The exact decimal expansion: a double has at most `significandWidth - exponent`
        // fractional binary digits, and as many fractional decimal ones.
        let fraction = magnitude == 0 ? 0 : Swift.max(0, magnitude.significandWidth - Int(magnitude.exponent))
        let exact = String(format: "%.\(fraction)f", magnitude)
        var integer = Array(exact.utf8.prefix { $0 != UInt8(ascii: ".") })
        var decimals = Array(exact.utf8.drop { $0 != UInt8(ascii: ".") }.dropFirst())
        while decimals.count <= digits { decimals.append(UInt8(ascii: "0")) }
        let next = decimals[digits]
        var kept = integer + decimals[..<digits]
        if next >= UInt8(ascii: "5") {
            var index = kept.count - 1
            while index >= 0 {
                if kept[index] == UInt8(ascii: "9") {
                    kept[index] = UInt8(ascii: "0")
                    index -= 1
                } else {
                    kept[index] += 1
                    break
                }
            }
            if index < 0 { kept.insert(UInt8(ascii: "1"), at: 0) }
        }
        integer = Array(kept[..<(kept.count - digits)])
        while integer.count > 1 && integer[0] == UInt8(ascii: "0") { integer.removeFirst() }
        if integer.isEmpty { integer = [UInt8(ascii: "0")] }
        let tail = kept[(kept.count - digits)...]
        let body = String(decoding: integer, as: UTF8.self)
        return digits == 0 ? sign + body : "\(sign)\(body).\(String(decoding: tail, as: UTF8.self))"
    }

    // MARK: Number to string

    /// `String(value)`: the shortest digits that read back as the same double, laid out as
    /// ECMAScript's Number::toString does (plain from 1e-6 up to 1e21, exponential outside).
    static func numberString(_ value: Double) -> String {
        if value.isNaN { return "NaN" }
        if value == 0 { return "0" }
        if value.isInfinite { return value < 0 ? "-Infinity" : "Infinity" }
        if value < 0 { return "-" + numberString(-value) }
        let (digits, point) = shortestDigits(value)
        let k = digits.count
        let n = point
        if k <= n && n <= 21 {
            return digits + String(repeating: "0", count: n - k)
        }
        if 0 < n && n <= 21 {
            let index = digits.index(digits.startIndex, offsetBy: n)
            return "\(digits[..<index]).\(digits[index...])"
        }
        if -6 < n && n <= 0 {
            return "0." + String(repeating: "0", count: -n) + digits
        }
        let exponent = n - 1
        let sign = exponent < 0 ? "-" : "+"
        let head = digits.prefix(1)
        let rest = digits.dropFirst()
        return rest.isEmpty
            ? "\(head)e\(sign)\(Swift.abs(exponent))"
            : "\(head).\(rest)e\(sign)\(Swift.abs(exponent))"
    }

    /// The shortest round-tripping digits of a positive finite double, without leading or
    /// trailing zeros, and where the decimal point goes: value = 0.digits × 10^point.
    private static func shortestDigits(_ value: Double) -> (String, Int) {
        // Swift's description is the shortest round-tripping form too, as "1.2345e+20",
        // "123.0" or "0.0001".
        let text = value.description
        var mantissa = Substring(text)
        var exponent = 0
        if let e = text.firstIndex(where: { $0 == "e" || $0 == "E" }) {
            mantissa = text[..<e]
            exponent = Int(text[text.index(after: e)...]) ?? 0
        }
        var digits = ""
        var point = 0
        var seenPoint = false
        for char in mantissa {
            if char == "." {
                seenPoint = true
            } else {
                digits.append(char)
                if !seenPoint { point += 1 }
            }
        }
        while digits.hasPrefix("0") {
            digits.removeFirst()
            point -= 1
        }
        while digits.hasSuffix("0") { digits.removeLast() }
        return (digits, point + exponent)
    }

    // MARK: String to number

    /// `Number(text)`: ECMAScript's StringToNumber. Whitespace around the number is ignored, the
    /// empty string is 0, and anything else that is not a numeric literal is NaN.
    static func toNumber(_ text: String) -> Double {
        let units = trim(text.units)
        if units.isEmpty { return 0 }
        let ascii = units.allSatisfy { $0 < 0x80 }
        guard ascii else { return .nan }
        let string = String(units: units)
        switch string {
        case "Infinity", "+Infinity": return .infinity
        case "-Infinity": return -.infinity
        default: break
        }
        if units.count > 2 && units[0] == 0x30 {
            switch units[1] {
            case 0x78, 0x58: return radixInteger(units.dropFirst(2), bits: 4)
            case 0x6F, 0x4F: return radixInteger(units.dropFirst(2), bits: 3)
            case 0x62, 0x42: return radixInteger(units.dropFirst(2), bits: 1)
            default: break
            }
        }
        guard isDecimalLiteral(units) else { return .nan }
        return Double(string) ?? .nan
    }

    /// StrDecimalLiteral without Infinity: `[+-]? (digits [. digits?] | . digits) ([eE] [+-]? digits)?`.
    private static func isDecimalLiteral(_ units: Units) -> Bool {
        var index = 0
        if index < units.count && (units[index] == 0x2B || units[index] == 0x2D) { index += 1 }
        var integerDigits = 0
        while index < units.count && isDigit(units[index]) {
            index += 1
            integerDigits += 1
        }
        var fractionDigits = 0
        if index < units.count && units[index] == 0x2E {
            index += 1
            while index < units.count && isDigit(units[index]) {
                index += 1
                fractionDigits += 1
            }
        }
        guard integerDigits + fractionDigits > 0 else { return false }
        if index < units.count && (units[index] == 0x65 || units[index] == 0x45) {
            index += 1
            if index < units.count && (units[index] == 0x2B || units[index] == 0x2D) { index += 1 }
            var exponentDigits = 0
            while index < units.count && isDigit(units[index]) {
                index += 1
                exponentDigits += 1
            }
            guard exponentDigits > 0 else { return false }
        }
        return index == units.count
    }

    /// A `0x`, `0o` or `0b` literal's digits, rounded to the nearest double (halves to even).
    private static func radixInteger(_ digits: ArraySlice<UInt16>, bits: Int) -> Double {
        var value: [Bool] = []
        for unit in digits {
            let digit: Int
            switch unit {
            case 0x30...0x39: digit = Int(unit - 0x30)
            case 0x61...0x66: digit = Int(unit - 0x61) + 10
            case 0x41...0x46: digit = Int(unit - 0x41) + 10
            default: return .nan
            }
            guard digit < 1 << bits else { return .nan }
            for shift in stride(from: bits - 1, through: 0, by: -1) {
                value.append(digit >> shift & 1 == 1)
            }
        }
        guard let first = value.firstIndex(of: true) else { return 0 }
        let significant = Array(value[first...])
        if significant.count <= 53 {
            var result = 0.0
            for bit in significant { result = result * 2 + (bit ? 1 : 0) }
            return result
        }
        var mantissa: UInt64 = 0
        for bit in significant.prefix(53) { mantissa = mantissa << 1 | (bit ? 1 : 0) }
        let rest = significant.dropFirst(53)
        let half = rest.first ?? false
        let sticky = rest.dropFirst().contains(true)
        if half && (sticky || mantissa & 1 == 1) { mantissa += 1 }
        let exponent = significant.count - 53
        return Double(sign: .plus, exponent: exponent, significand: Double(mantissa))
    }
}
