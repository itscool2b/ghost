public import CoreGraphics
public import O7Theme
public import SwiftUI

/// The o7 wing (logoPaths.ts): a flying wing seen from above with two eyes, fitted to the rect it
/// is given and centred, far wider than tall. The eyes are holes whatever the fill rule. The small
/// variant's larger eyes stay open at icon sizes: use it at `smallEyesMaxWidth` and below.
public struct O7Logo: Shape {
    public var smallEyes: Bool

    public init(smallEyes: Bool = false) {
        self.smallEyes = smallEyes
    }

    /// The wing for a drawn width: the small variant at `smallEyesMaxWidth` and below.
    public init(width: CGFloat) {
        self.init(smallEyes: width <= Self.smallEyesMaxWidth)
    }

    /// Width over height of the wing's own box (1044 by 440).
    public static let aspectRatio: CGFloat = O7LogoPaths.viewBox.width / O7LogoPaths.viewBox.height

    /// The widest the wing is drawn with the small variant's eyes.
    public static let smallEyesMaxWidth: CGFloat = O7LogoPaths.smallMaxWidth

    public func path(in rect: CGRect) -> Path {
        let box = O7LogoPaths.viewBox
        let k = min(rect.width / box.width, rect.height / box.height)
        let x = rect.midX - box.width * k / 2
        let y = rect.midY - box.height * k / 2
        let unit = smallEyes ? Self.small : Self.full
        return unit.applying(CGAffineTransform(a: k, b: 0, c: 0, d: k, tx: x, ty: y))
    }

    public func sizeThatFits(_ proposal: ProposedViewSize) -> CGSize {
        let box = O7LogoPaths.viewBox
        switch (proposal.width, proposal.height) {
        case let (width?, height?):
            let k = min(width / box.width, height / box.height)
            return CGSize(width: box.width * k, height: box.height * k)
        case let (width?, nil):
            return CGSize(width: width, height: width / Self.aspectRatio)
        case let (nil, height?):
            return CGSize(width: height * Self.aspectRatio, height: height)
        case (nil, nil):
            return box
        }
    }

    /// The outline and eyes in the wing's own coordinates, as one even-odd shape normalised so
    /// the default fill rule draws the eyes as holes too.
    private static let full = unit(eyes: O7LogoPaths.eyes)
    private static let small = unit(eyes: O7LogoPaths.smallEyes)

    private static func unit(eyes: [String]) -> Path {
        var path = Path()
        for data in [O7LogoPaths.outline] + eyes {
            if let part = try? SVGPathParser.path(data) { path.addPath(part) }
        }
        return path.normalized(eoFill: true)
    }
}

/// "o7" in Geist 600 with tight tracking (-0.035em), after the wing (Logo.tsx's Wordmark). The
/// wing is not quite twice as wide as the type is tall, about as tall as a lowercase letter, and
/// centred on the type. It reads as "o7".
public struct Wordmark: View {
    private let size: CGFloat
    private let mark: Bool
    @Environment(\.o7Palette) private var palette

    /// The word at `size` points (10 to 320), with the wing before it unless `mark` is false.
    public init(size: CGFloat = 16, mark: Bool = true) {
        self.size = size
        self.mark = mark
    }

    public var body: some View {
        let fontSize = size.isFinite ? max(10, min(320, size)) : 16
        HStack(alignment: .center, spacing: 0.375 * fontSize) {
            if mark {
                let width = CGFloat(max(8, min(1024, PixelGeometry.jsRound(Double(fontSize) * 1.9))))
                let height = CGFloat(max(4, PixelGeometry.jsRound(Double(width * O7LogoPaths.viewBox.height / O7LogoPaths.viewBox.width))))
                O7Logo(width: width)
                    .fill(Color(palette.fg1))
                    .frame(width: width, height: height)
            }
            Text(verbatim: "o7")
                .font(O7Font.fixed(fontSize, weight: .semibold))
                .tracking(-0.035 * fontSize)
                .lineLimit(1)
                .fixedSize()
        }
        .foregroundStyle(Color(palette.fg1))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: "o7"))
    }
}
