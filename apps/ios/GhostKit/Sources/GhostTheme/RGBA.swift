import Foundation

/// A colour in sRGB with straight alpha, each channel from 0 to 1, as the desktop's tokens write
/// them (`#rrggbb` or `rgb(r g b / a)`).
public struct RGBA: Sendable, Hashable {
    public var r: Double
    public var g: Double
    public var b: Double
    public var a: Double

    public init(r: Double, g: Double, b: Double, a: Double = 1) {
        self.r = r
        self.g = g
        self.b = b
        self.a = a
    }

    /// A colour from `0xrrggbb` and an alpha.
    public init(hex: UInt32, alpha: Double = 1) {
        self.init(
            r: Double((hex >> 16) & 0xff) / 255,
            g: Double((hex >> 8) & 0xff) / 255,
            b: Double(hex & 0xff) / 255,
            a: alpha
        )
    }

    /// Parses `#rrggbb`, `#rgb` or `rgb(r g b / a)`, the forms the desktop's tokens use.
    public init?(css: String) {
        let text = css.trimmingCharacters(in: .whitespaces)
        if text.hasPrefix("#") {
            var digits = String(text.dropFirst())
            if digits.count == 3 { digits = digits.map { "\($0)\($0)" }.joined() }
            guard digits.count == 6, let value = UInt32(digits, radix: 16) else { return nil }
            self.init(hex: value)
            return
        }
        guard text.hasPrefix("rgb("), text.hasSuffix(")") else { return nil }
        let inner = text.dropFirst(4).dropLast()
        let halves = inner.split(separator: "/", omittingEmptySubsequences: false)
        guard halves.count <= 2 else { return nil }
        let channels = halves[0].split(separator: " ").compactMap { Double($0) }
        guard channels.count == 3, channels.allSatisfy({ (0...255).contains($0) }) else { return nil }
        var alpha = 1.0
        if halves.count == 2 {
            guard let value = Double(halves[1].trimmingCharacters(in: .whitespaces)), (0...1).contains(value)
            else { return nil }
            alpha = value
        }
        self.init(r: channels[0] / 255, g: channels[1] / 255, b: channels[2] / 255, a: alpha)
    }

    /// The colour as `#rrggbb`, alpha left out.
    public var hex: String {
        let q = quantized
        let value = (Int(q.r * 255) << 16) | (Int(q.g * 255) << 8) | Int(q.b * 255)
        let digits = String(value, radix: 16)
        return "#" + String(repeating: "0", count: 6 - digits.count) + digits
    }

    /// The same colour with a different alpha.
    public func opacity(_ alpha: Double) -> RGBA {
        RGBA(r: r, g: g, b: b, a: alpha)
    }

    /// Channels rounded to 8 bits, as a display draws them.
    public var quantized: RGBA {
        func round8(_ value: Double) -> Double { (min(1, max(0, value)) * 255).rounded() / 255 }
        return RGBA(r: round8(r), g: round8(g), b: round8(b), a: a)
    }

    /// This colour drawn over `under` (source over).
    public func over(_ under: RGBA) -> RGBA {
        let alpha = a + under.a * (1 - a)
        guard alpha > 0 else { return RGBA(r: 0, g: 0, b: 0, a: 0) }
        func blend(_ top: Double, _ bottom: Double) -> Double {
            (top * a + bottom * under.a * (1 - a)) / alpha
        }
        return RGBA(r: blend(r, under.r), g: blend(g, under.g), b: blend(b, under.b), a: alpha)
    }

    /// CSS `color-mix(in srgb, self <amount>, other)`: `amount` of this colour, the rest of
    /// `other`, mixed with premultiplied alpha as CSS does.
    public func mixed(_ amount: Double, into other: RGBA) -> RGBA {
        let p = min(1, max(0, amount))
        let alpha = a * p + other.a * (1 - p)
        guard alpha > 0 else { return RGBA(r: 0, g: 0, b: 0, a: 0) }
        func blend(_ mine: Double, _ theirs: Double) -> Double {
            (mine * a * p + theirs * other.a * (1 - p)) / alpha
        }
        return RGBA(r: blend(r, other.r), g: blend(g, other.g), b: blend(b, other.b), a: alpha)
    }

    /// WCAG relative luminance, ignoring alpha.
    public var luminance: Double {
        func linear(_ value: Double) -> Double {
            value <= 0.04045 ? value / 12.92 : pow((value + 0.055) / 1.055, 2.4)
        }
        return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    /// WCAG contrast ratio with another colour, from 1 to 21, ignoring alpha: composite first.
    public func contrast(with other: RGBA) -> Double {
        let (x, y) = (luminance, other.luminance)
        return (max(x, y) + 0.05) / (min(x, y) + 0.05)
    }
}

extension RGBA: CustomStringConvertible {
    public var description: String {
        a == 1 ? hex : "\(hex) / \(a)"
    }
}
