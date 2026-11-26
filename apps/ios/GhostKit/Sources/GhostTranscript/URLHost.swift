internal import Foundation

// `new URL(url).hostname` for the http and https links `safeHref` lets through, by the WHATWG URL
// Standard's parser: the authority after any run of slashes, credentials before its last "@", a
// port that must be a number up to 65535, IPv6 in brackets, percent-decoding, IPv4 in any of its
// forms (0x7f.1 is 127.0.0.1), and a domain lowercased and Punycode-encoded.
//
// A divergence: a domain with non-ASCII letters is lowercased and normalized (NFC) here rather
// than mapped through UTS 46's full table, so the rare characters that table maps otherwise (or
// forbids) read differently. ASCII hosts, which web search results give, are exact.

enum URLHost {
    /// The hostname of an http or https URL, or nil where `new URL` throws.
    static func hostname(_ url: String) -> String? {
        var input = url.units
        // Leading and trailing C0 controls and spaces go; tabs and newlines go wherever they are.
        while let first = input.first, first <= 0x20 { input.removeFirst() }
        while let last = input.last, last <= 0x20 { input.removeLast() }
        input.removeAll { $0 == 0x09 || $0 == 0x0A || $0 == 0x0D }
        guard let colon = input.firstIndex(of: 0x3A) else { return nil }
        let scheme = String(units: input[..<colon]).lowercased()
        guard scheme == "http" || scheme == "https" else { return nil }
        var at = colon + 1
        // Special authority slashes: any run of / and \ before the authority.
        while at < input.count && (input[at] == 0x2F || input[at] == 0x5C) { at += 1 }
        var end = at
        while end < input.count, ![0x2F, 0x5C, 0x3F, 0x23].contains(input[end]) { end += 1 }
        var authority = Array(input[at..<end])
        if let sign = authority.lastIndex(of: 0x40) {
            authority = Array(authority[(sign + 1)...])
            if authority.isEmpty { return nil }
        }
        // The port starts at the first ":" outside brackets.
        var hostEnd = authority.count
        var bracket = false
        for (index, unit) in authority.enumerated() {
            if unit == 0x5B { bracket = true }
            if unit == 0x5D { bracket = false }
            if unit == 0x3A && !bracket {
                hostEnd = index
                break
            }
        }
        let host = Array(authority[..<hostEnd])
        if hostEnd < authority.count {
            let port = authority[(hostEnd + 1)...]
            guard port.allSatisfy(JS.isDigit) else { return nil }
            var value = 0
            for digit in port {
                value = value * 10 + Int(digit - 0x30)
                if value > 65535 { return nil }
            }
        }
        if host.isEmpty { return nil }
        return parseHost(host)
    }

    private static func parseHost(_ input: Units) -> String? {
        if input.first == 0x5B {
            guard input.last == 0x5D, input.count >= 2,
                let address = ipv6(Array(input[1..<(input.count - 1)]))
            else { return nil }
            return "[\(serializeIPv6(address))]"
        }
        guard let decoded = percentDecoded(input), let ascii = domainToASCII(decoded) else { return nil }
        let units = ascii.units
        if units.contains(where: isForbiddenDomainUnit) { return nil }
        if endsInNumber(units) {
            guard let address = ipv4(units) else { return nil }
            return serializeIPv4(address)
        }
        return ascii
    }

    // MARK: Domains

    private static func isForbiddenDomainUnit(_ unit: UInt16) -> Bool {
        switch unit {
        case 0x00...0x20, 0x23, 0x25, 0x2F, 0x3A, 0x3C, 0x3E, 0x3F, 0x40, 0x5B, 0x5C, 0x5D, 0x5E,
            0x7C, 0x7F:
            return true
        default:
            return false
        }
    }

    /// UTF-8 percent-decoding; nil when the bytes are not UTF-8.
    private static func percentDecoded(_ input: Units) -> String? {
        var bytes: [UInt8] = []
        for unit in String(units: input).utf8 { bytes.append(unit) }
        var out: [UInt8] = []
        var index = 0
        while index < bytes.count {
            let byte = bytes[index]
            if byte == 0x25, index + 2 < bytes.count, let high = hex(bytes[index + 1]),
                let low = hex(bytes[index + 2])
            {
                out.append(high << 4 | low)
                index += 3
            } else {
                out.append(byte)
                index += 1
            }
        }
        // The standard decodes without failure, replacing bad sequences, which then fail the domain.
        return String(bytes: out, encoding: .utf8) ?? String(decoding: out, as: UTF8.self)
    }

    private static func hex(_ byte: UInt8) -> UInt8? {
        switch byte {
        case 0x30...0x39: byte - 0x30
        case 0x61...0x66: byte - 0x61 + 10
        case 0x41...0x46: byte - 0x41 + 10
        default: nil
        }
    }

    /// UTS 46 ToASCII as the URL Standard calls it, without the full mapping table: ideographic and
    /// fullwidth full stops separate labels, letters are lowercased and normalized, and a label with
    /// non-ASCII letters is Punycode-encoded.
    private static func domainToASCII(_ domain: String) -> String? {
        var mapped = ""
        for scalar in domain.unicodeScalars {
            switch scalar.value {
            case 0xFFFD: return nil
            case 0x3002, 0xFF0E, 0xFF61: mapped.append(".")
            case 0x41...0x5A: mapped.unicodeScalars.append(Unicode.Scalar(scalar.value + 0x20)!)
            case 0x00...0x7F: mapped.unicodeScalars.append(scalar)
            default: mapped.append(contentsOf: String(scalar).lowercased())
            }
        }
        mapped = mapped.precomposedStringWithCanonicalMapping
        if mapped.unicodeScalars.allSatisfy(\.isASCII) {
            // A label Punycode encoded must decode.
            for label in mapped.split(separator: ".", omittingEmptySubsequences: false)
            where label.hasPrefix("xn--") {
                guard punycodeDecodes(label.dropFirst(4)) else { return nil }
            }
            return mapped
        }
        var labels: [String] = []
        for label in mapped.split(separator: ".", omittingEmptySubsequences: false) {
            if label.unicodeScalars.allSatisfy(\.isASCII) {
                labels.append(String(label))
            } else {
                guard let encoded = punycode(String(label)) else { return nil }
                labels.append("xn--" + encoded)
            }
        }
        return labels.joined(separator: ".")
    }

    /// RFC 3492's encoder.
    private static func punycode(_ label: String) -> String? {
        let base = 36
        let tMin = 1
        let tMax = 26
        let skew = 38
        let damp = 700
        let input = label.unicodeScalars.map { Int($0.value) }
        var output = input.filter { $0 < 0x80 }.map { Character(Unicode.Scalar(UInt8($0))) }
        let basic = output.count
        var handled = basic
        if basic > 0 { output.append("-") }
        var n = 0x80
        var delta = 0
        var bias = 72
        func digit(_ value: Int) -> Character {
            Character(Unicode.Scalar(UInt8(value < 26 ? value + 0x61 : value - 26 + 0x30)))
        }
        func adapt(_ delta: Int, _ points: Int, _ first: Bool) -> Int {
            var delta = first ? delta / damp : delta / 2
            delta += delta / points
            var k = 0
            while delta > ((base - tMin) * tMax) / 2 {
                delta /= base - tMin
                k += base
            }
            return k + (base - tMin + 1) * delta / (delta + skew)
        }
        while handled < input.count {
            guard let m = input.filter({ $0 >= n }).min() else { return nil }
            delta += (m - n) * (handled + 1)
            n = m
            for code in input {
                if code < n { delta += 1 }
                if code == n {
                    var q = delta
                    var k = base
                    while true {
                        let t = k <= bias ? tMin : (k >= bias + tMax ? tMax : k - bias)
                        if q < t { break }
                        output.append(digit(t + (q - t) % (base - t)))
                        q = (q - t) / (base - t)
                        k += base
                    }
                    output.append(digit(q))
                    bias = adapt(delta, handled + 1, handled == basic)
                    delta = 0
                    handled += 1
                }
            }
            delta += 1
            n += 1
        }
        return String(output)
    }

    /// Whether a Punycode label's digits decode (RFC 3492's decoder, run for its failures).
    private static func punycodeDecodes(_ encoded: Substring) -> Bool {
        let base = 36
        let scalars = Array(encoded.unicodeScalars)
        if scalars.isEmpty { return false }
        let dash = scalars.lastIndex(of: "-") ?? -1
        var index = dash + 1
        var i = 0
        var n = 0x80
        var bias = 72
        var length = Swift.max(dash, 0)
        while index < scalars.count {
            let old = i
            var w = 1
            var k = base
            while true {
                guard index < scalars.count else { return false }
                let code = Int(scalars[index].value)
                index += 1
                let digit: Int
                switch code {
                case 0x30...0x39: digit = code - 0x30 + 26
                case 0x41...0x5A: digit = code - 0x41
                case 0x61...0x7A: digit = code - 0x61
                default: return false
                }
                i += digit * w
                let t = k <= bias ? 1 : (k >= bias + 26 ? 26 : k - bias)
                if digit < t { break }
                w *= base - t
                k += base
                if i > 0x10FFFF * 64 || w > 0x10FFFF * 64 { return false }
            }
            length += 1
            var delta = old == 0 ? (i - old) / 700 : (i - old) / 2
            delta += delta / length
            var kk = 0
            while delta > 455 {
                delta /= 35
                kk += base
            }
            bias = kk + 36 * delta / (delta + 38)
            n += i / length
            i %= length
            if n > 0x10FFFF { return false }
            i += 1
        }
        return true
    }

    // MARK: IPv4

    /// The URL Standard's "ends in a number" check.
    private static func endsInNumber(_ host: Units) -> Bool {
        var parts = JS.split(host, [0x2E])
        if parts.last?.isEmpty == true {
            if parts.count == 1 { return false }
            parts.removeLast()
        }
        guard let last = parts.last, !last.isEmpty else { return false }
        if last.allSatisfy(JS.isDigit) { return true }
        return ipv4Number(last) != nil
    }

    private static func ipv4(_ host: Units) -> UInt32? {
        var parts = JS.split(host, [0x2E])
        if parts.last?.isEmpty == true, parts.count > 1 { parts.removeLast() }
        guard parts.count <= 4 else { return nil }
        var numbers: [UInt64] = []
        for part in parts {
            guard let number = ipv4Number(part) else { return nil }
            numbers.append(number)
        }
        for number in numbers.dropLast() where number > 255 { return nil }
        guard let last = numbers.last, last < UInt64(1) << (8 * (5 - numbers.count)) else { return nil }
        var address = last
        for (index, number) in numbers.dropLast().enumerated() {
            address += number << (8 * (3 - index))
        }
        return UInt32(address)
    }

    /// One part of an IPv4 address: decimal, 0x hexadecimal or 0 octal. Values past 2^40 are
    /// capped: any of them fails the address anyway.
    private static func ipv4Number(_ part: Units) -> UInt64? {
        if part.isEmpty { return nil }
        var digits = part[...]
        var radix: UInt64 = 10
        if digits.count >= 2, digits.first == 0x30, digits.dropFirst().first == 0x78 || digits.dropFirst().first == 0x58 {
            digits = digits.dropFirst(2)
            radix = 16
        } else if digits.count >= 2, digits.first == 0x30 {
            digits = digits.dropFirst()
            radix = 8
        }
        if digits.isEmpty { return 0 }
        var value: UInt64 = 0
        for unit in digits {
            let digit: UInt64
            switch unit {
            case 0x30...0x39: digit = UInt64(unit - 0x30)
            case 0x61...0x66: digit = UInt64(unit - 0x61 + 10)
            case 0x41...0x46: digit = UInt64(unit - 0x41 + 10)
            default: return nil
            }
            guard digit < radix else { return nil }
            value = Swift.min(value * radix + digit, UInt64(1) << 41)
        }
        return value
    }

    private static func serializeIPv4(_ address: UInt32) -> String {
        (0..<4).map { String(address >> (8 * (3 - $0)) & 0xFF) }.joined(separator: ".")
    }

    // MARK: IPv6

    private static func ipv6(_ input: Units) -> [UInt16]? {
        var address = [UInt16](repeating: 0, count: 8)
        var piece = 0
        var compress: Int?
        var at = 0
        if at < input.count && input[at] == 0x3A {
            guard at + 1 < input.count, input[at + 1] == 0x3A else { return nil }
            at += 2
            piece += 1
            compress = piece
        }
        while at < input.count {
            if piece == 8 { return nil }
            if input[at] == 0x3A {
                if compress != nil { return nil }
                at += 1
                piece += 1
                compress = piece
                continue
            }
            var value: UInt16 = 0
            var length = 0
            while length < 4, at < input.count, let digit = hexUnit(input[at]) {
                value = value * 0x10 + digit
                at += 1
                length += 1
            }
            if at < input.count && input[at] == 0x2E {
                if length == 0 { return nil }
                at -= length
                if piece > 6 { return nil }
                var seen = 0
                while at < input.count {
                    var ipv4Piece: Int?
                    if seen > 0 {
                        if input[at] == 0x2E && seen < 4 { at += 1 } else { return nil }
                    }
                    guard at < input.count, JS.isDigit(input[at]) else { return nil }
                    while at < input.count, JS.isDigit(input[at]) {
                        let number = Int(input[at] - 0x30)
                        if let current = ipv4Piece {
                            if current == 0 { return nil }
                            ipv4Piece = current * 10 + number
                        } else {
                            ipv4Piece = number
                        }
                        if ipv4Piece! > 255 { return nil }
                        at += 1
                    }
                    address[piece] = address[piece] * 0x100 + UInt16(ipv4Piece!)
                    seen += 1
                    if seen == 2 || seen == 4 { piece += 1 }
                }
                if seen != 4 { return nil }
                break
            } else if at < input.count && input[at] == 0x3A {
                at += 1
                if at >= input.count { return nil }
            } else if at < input.count {
                return nil
            }
            address[piece] = value
            piece += 1
        }
        if let compress {
            var swaps = piece - compress
            piece = 7
            while piece != 0 && swaps > 0 {
                address.swapAt(piece, compress + swaps - 1)
                piece -= 1
                swaps -= 1
            }
        } else if piece != 8 {
            return nil
        }
        return address
    }

    private static func hexUnit(_ unit: UInt16) -> UInt16? {
        switch unit {
        case 0x30...0x39: unit - 0x30
        case 0x61...0x66: unit - 0x61 + 10
        case 0x41...0x46: unit - 0x41 + 10
        default: nil
        }
    }

    /// The URL Standard's IPv6 serializer: the first longest run of two or more zero pieces
    /// becomes "::".
    private static func serializeIPv6(_ address: [UInt16]) -> String {
        var best: (start: Int, length: Int)?
        var index = 0
        while index < 8 {
            if address[index] == 0 {
                var end = index
                while end < 8 && address[end] == 0 { end += 1 }
                let length = end - index
                if length >= 2 && length > (best?.length ?? 0) { best = (index, length) }
                index = end
            } else {
                index += 1
            }
        }
        var out = ""
        var ignore0 = false
        for piece in 0..<8 {
            if ignore0 && address[piece] == 0 { continue }
            ignore0 = false
            if let best, best.start == piece {
                out += piece == 0 ? "::" : ":"
                ignore0 = true
                continue
            }
            out += String(address[piece], radix: 16)
            if piece != 7 { out += ":" }
        }
        return out
    }
}
