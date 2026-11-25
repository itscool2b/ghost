public import CoreGraphics
public import SwiftUI

/// Reads SVG path data into a SwiftUI `Path`. It takes the commands the stored marks and the wing
/// use, M L H V C Z and their relative forms, with SVG's number grammar (signs, leading dots,
/// exponents, numbers run together as in `l.2.4`), commas or whitespace between numbers, and
/// repeated arguments (after M they are lines). Anything else is an error, never a guess.
public enum SVGPathParser {
    /// Why path data was refused, and where.
    public struct Failure: Error, Sendable, Equatable, CustomStringConvertible {
        /// The byte offset in the UTF-8 path data.
        public let offset: Int
        public let reason: String

        public var description: String { "SVG path, byte \(offset): \(reason)" }
    }

    /// One absolute drawing step.
    public enum Command: Sendable, Equatable {
        case move(CGPoint)
        case line(CGPoint)
        case curve(to: CGPoint, control1: CGPoint, control2: CGPoint)
        case close
    }

    /// The path data as a `Path`.
    public static func path(_ data: String) throws -> Path {
        var path = Path()
        for command in try commands(data) {
            switch command {
            case .move(let point): path.move(to: point)
            case .line(let point): path.addLine(to: point)
            case .curve(let to, let control1, let control2): path.addCurve(to: to, control1: control1, control2: control2)
            case .close: path.closeSubpath()
            }
        }
        return path
    }

    /// The path data as absolute commands.
    public static func commands(_ data: String) throws -> [Command] {
        var reader = Reader(bytes: Array(data.utf8))
        var commands: [Command] = []
        var current = CGPoint.zero
        var start = CGPoint.zero
        var previous: UInt8?
        reader.skipSeparators()
        while !reader.atEnd {
            let letter: UInt8
            if let next = reader.peek, isCommand(next) {
                letter = next
                reader.index += 1
            } else if let repeated = previous {
                // Repeated arguments: after a move they are lines.
                letter = repeated == ascii("M") ? ascii("L") : repeated == ascii("m") ? ascii("l") : repeated
                if letter == ascii("Z") || letter == ascii("z") {
                    throw Failure(offset: reader.index, reason: "numbers after a close")
                }
            } else {
                throw Failure(offset: reader.index, reason: "path data must start with a move")
            }
            if previous == nil, letter != ascii("M"), letter != ascii("m") {
                throw Failure(offset: reader.index - 1, reason: "path data must start with a move")
            }
            let relative = letter >= ascii("a")
            func point(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
                relative ? CGPoint(x: current.x + x, y: current.y + y) : CGPoint(x: x, y: y)
            }
            switch letter | 0x20 {
            case ascii("m"):
                let (x, y) = (try reader.number(), try reader.number())
                current = point(x, y)
                start = current
                commands.append(.move(current))
            case ascii("l"):
                let (x, y) = (try reader.number(), try reader.number())
                current = point(x, y)
                commands.append(.line(current))
            case ascii("h"):
                let x = try reader.number()
                current = CGPoint(x: relative ? current.x + x : x, y: current.y)
                commands.append(.line(current))
            case ascii("v"):
                let y = try reader.number()
                current = CGPoint(x: current.x, y: relative ? current.y + y : y)
                commands.append(.line(current))
            case ascii("c"):
                let c1 = point(try reader.number(), try reader.number())
                let c2 = point(try reader.number(), try reader.number())
                let to = point(try reader.number(), try reader.number())
                current = to
                commands.append(.curve(to: to, control1: c1, control2: c2))
            case ascii("z"):
                current = start
                commands.append(.close)
            default:
                throw Failure(offset: reader.index - 1, reason: "unsupported command \(Character(Unicode.Scalar(letter)))")
            }
            previous = letter
            reader.skipSeparators()
        }
        return commands
    }

    private static func isCommand(_ byte: UInt8) -> Bool {
        (byte >= ascii("A") && byte <= ascii("Z")) || (byte >= ascii("a") && byte <= ascii("z"))
    }
}

private func ascii(_ character: Unicode.Scalar) -> UInt8 { UInt8(character.value) }

private struct Reader {
    let bytes: [UInt8]
    var index = 0

    var atEnd: Bool { index >= bytes.count }
    var peek: UInt8? { atEnd ? nil : bytes[index] }

    /// Whitespace with at most one comma among it.
    mutating func skipSeparators() {
        var comma = false
        while let byte = peek {
            if byte == 0x20 || byte == 0x09 || byte == 0x0A || byte == 0x0D || byte == 0x0C {
                index += 1
            } else if byte == ascii(","), !comma {
                comma = true
                index += 1
            } else {
                break
            }
        }
    }

    /// `[+-]? (digits [. digits?] | . digits) ([eE] [+-]? digits)?`, after any separators.
    mutating func number() throws -> CGFloat {
        skipSeparators()
        let begin = index
        if let sign = peek, sign == ascii("+") || sign == ascii("-") { index += 1 }
        let whole = digits()
        var fraction = 0
        if peek == ascii(".") {
            index += 1
            fraction = digits()
        }
        guard whole + fraction > 0 else {
            throw SVGPathParser.Failure(offset: begin, reason: "expected a number")
        }
        if let e = peek, e == ascii("e") || e == ascii("E") {
            let mark = index
            index += 1
            if let sign = peek, sign == ascii("+") || sign == ascii("-") { index += 1 }
            // An `e` without digits is no exponent; the letter is then refused as a command.
            if digits() == 0 { index = mark }
        }
        let text = String(decoding: bytes[begin..<index], as: UTF8.self)
        guard let value = Double(text), value.isFinite else {
            throw SVGPathParser.Failure(offset: begin, reason: "bad number \(text)")
        }
        return CGFloat(value)
    }

    private mutating func digits() -> Int {
        let begin = index
        while let byte = peek, byte >= ascii("0"), byte <= ascii("9") { index += 1 }
        return index - begin
    }
}
