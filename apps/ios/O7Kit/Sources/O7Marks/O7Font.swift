import CoreText
import Foundation
public import SwiftUI
import Synchronization
#if canImport(UIKit)
import UIKit
#endif

/// o7's three weights, on the faces' variable `wght` axis at the grade the desktop takes on
/// macOS, where Geist reads a touch light at 400.
public enum O7Weight: Sendable, CaseIterable {
    case regular
    case medium
    case semibold

    /// The `wght` axis value: 420, 510 or 600.
    public var value: CGFloat {
        switch self {
        case .regular: 420
        case .medium: 510
        case .semibold: 600
        }
    }
}

/// A role in o7's type scale: size and line height in points, tracking in em, weight and face,
/// and the text style whose Dynamic Type curve it follows.
public struct O7TextStyle: Sendable, Hashable {
    public var size: CGFloat
    public var lineHeight: CGFloat
    /// Letter spacing as a fraction of the size, as CSS `em` (negative tightens).
    public var tracking: CGFloat
    public var weight: O7Weight
    public var mono: Bool
    /// Tabular figures, so times and counts do not jitter.
    public var tabular: Bool
    public var relativeTo: Font.TextStyle

    public init(
        size: CGFloat, lineHeight: CGFloat, tracking: CGFloat = 0, weight: O7Weight = .regular,
        mono: Bool = false, tabular: Bool = false, relativeTo: Font.TextStyle
    ) {
        self.size = size
        self.lineHeight = lineHeight
        self.tracking = tracking
        self.weight = weight
        self.mono = mono
        self.tabular = tabular
        self.relativeTo = relativeTo
    }

    // The desktop's scale (tokens.css).
    /// 11/16: keycaps and the smallest labels.
    public static let caption = O7TextStyle(size: 11, lineHeight: 16, relativeTo: .caption2)
    /// 12/16: the desktop's meta line.
    public static let small = O7TextStyle(size: 12, lineHeight: 16, relativeTo: .caption)
    /// 13/20: the desktop's interface text.
    public static let interface = O7TextStyle(size: 13, lineHeight: 20, relativeTo: .footnote)
    /// 14/22: the desktop's chat text.
    public static let chat = O7TextStyle(size: 14, lineHeight: 22, relativeTo: .subheadline)
    /// 15/24: prose, the phone's body.
    public static let prose = O7TextStyle(size: 15, lineHeight: 24, relativeTo: .subheadline)
    /// 17/24 semibold.
    public static let heading = O7TextStyle(
        size: 17, lineHeight: 24, tracking: -0.008, weight: .semibold, relativeTo: .headline)
    /// 22/28 medium.
    public static let title = O7TextStyle(
        size: 22, lineHeight: 28, tracking: -0.012, weight: .medium, relativeTo: .title2)
    /// 26/32 medium.
    public static let display = O7TextStyle(
        size: 26, lineHeight: 32, tracking: -0.015, weight: .medium, relativeTo: .title)
    /// 13/20 Geist Mono: code blocks and inline code.
    public static let code = O7TextStyle(size: 13, lineHeight: 20, mono: true, relativeTo: .footnote)
    /// 12.5/18 Geist Mono: terminal output.
    public static let terminal = O7TextStyle(size: 12.5, lineHeight: 18, mono: true, relativeTo: .caption)

    // The phone remote's scale (apps/web/app/styles/remote.css).
    /// 15/21 medium: a chat row's title.
    public static let rowTitle = O7TextStyle(size: 15, lineHeight: 21, weight: .medium, relativeTo: .subheadline)
    /// 13/18: meta lines, in fg-3.
    public static let meta = O7TextStyle(size: 13, lineHeight: 18, relativeTo: .footnote)
    /// 12/16 with tabular figures: times and elapsed counters.
    public static let time = O7TextStyle(size: 12, lineHeight: 16, tabular: true, relativeTo: .caption)
    /// 16/22: the composer.
    public static let composer = O7TextStyle(size: 16, lineHeight: 22, relativeTo: .callout)
    /// 18/24 medium: a sheet's title.
    public static let sheetTitle = O7TextStyle(size: 18, lineHeight: 24, weight: .medium, relativeTo: .title3)
    /// 16/22 medium: a bar's title.
    public static let barTitle = O7TextStyle(size: 16, lineHeight: 22, weight: .medium, relativeTo: .callout)
}

/// SwiftUI fonts in Geist and Geist Mono at o7's weights, scaled with Dynamic Type. A font is
/// sized for the text size in effect when it is made; `View.o7Font(_:)` also follows a change
/// while the view is on screen and sets the style's line height and tracking.
public enum O7Font {
    /// Geist at `size`, scaled by SwiftUI as `relativeTo` scales, live, as Dynamic Type asks.
    public static func ui(_ size: CGFloat, weight: O7Weight = .regular, relativeTo: Font.TextStyle = .body) -> Font {
        dynamic(size: size, weight: weight, mono: false, tabular: false, relativeTo: relativeTo, bold: false)
    }

    /// Geist Mono at `size`, scaled by SwiftUI as `relativeTo` scales.
    public static func mono(_ size: CGFloat, weight: O7Weight = .regular, relativeTo: Font.TextStyle = .body) -> Font {
        dynamic(size: size, weight: weight, mono: true, tabular: false, relativeTo: relativeTo, bold: false)
    }

    /// A face SwiftUI scales itself with Dynamic Type, by its named instance. Text Styles scale fonts
    /// this way, and accessibility checks recognise it; the `wght` axis values the desktop grades
    /// for its own screens (420, 510) round to the instances (400, 500) here. Bold Text takes the
    /// next heavier instance.
    static func dynamic(
        size: CGFloat, weight: O7Weight, mono: Bool, tabular: Bool, relativeTo: Font.TextStyle, bold: Bool
    ) -> Font {
        O7Fonts.register()
        let instance =
            switch (weight, bold) {
            case (.regular, false): "Regular"
            case (.medium, false): "Medium"
            case (.semibold, false): "SemiBold"
            case (.regular, true): "SemiBold"
            case (.medium, true): "Bold"
            case (.semibold, true): "ExtraBold"
            }
        let font = Font.custom("\(mono ? "GeistMono" : "Geist")-\(instance)", size: size, relativeTo: relativeTo)
        return tabular ? font.monospacedDigit() : font
    }

    /// A size that never scales: marks, the wordmark, anything drawn to a fixed box.
    public static func fixed(_ size: CGFloat, weight: O7Weight = .regular, mono: Bool = false) -> Font {
        font(size: size, weight: weight.value, mono: mono, tabular: false)
    }

    /// A role's font, scaled by SwiftUI with Dynamic Type.
    public static func font(_ style: O7TextStyle) -> Font {
        dynamic(
            size: style.size, weight: style.weight, mono: style.mono, tabular: style.tabular,
            relativeTo: style.relativeTo, bold: false)
    }

    /// A role's font at a given Dynamic Type size, with Bold Text on or off.
    public static func font(_ style: O7TextStyle, dynamicTypeSize: DynamicTypeSize, boldText: Bool = false) -> Font {
        font(
            size: scaled(style.size, relativeTo: style.relativeTo, dynamicTypeSize: dynamicTypeSize),
            weight: boldText ? bolder(style.weight.value) : style.weight.value, mono: style.mono, tabular: style.tabular)
    }

    public static var caption: Font { font(.caption) }
    public static var small: Font { font(.small) }
    public static var interface: Font { font(.interface) }
    public static var chat: Font { font(.chat) }
    public static var prose: Font { font(.prose) }
    public static var heading: Font { font(.heading) }
    public static var title: Font { font(.title) }
    public static var display: Font { font(.display) }
    public static var code: Font { font(.code) }
    public static var terminal: Font { font(.terminal) }
    public static var rowTitle: Font { font(.rowTitle) }
    public static var meta: Font { font(.meta) }
    public static var time: Font { font(.time) }
    public static var composer: Font { font(.composer) }
    public static var sheetTitle: Font { font(.sheetTitle) }
    public static var barTitle: Font { font(.barTitle) }

    /// `value` scaled for Dynamic Type as `relativeTo` scales: at the size in effect now, or at
    /// `dynamicTypeSize`. On a Mac it is `value` itself.
    public static func scaled(_ value: CGFloat, relativeTo: Font.TextStyle, dynamicTypeSize: DynamicTypeSize? = nil) -> CGFloat {
        #if canImport(UIKit)
        let metrics = UIFontMetrics(forTextStyle: uiTextStyle(relativeTo))
        guard let dynamicTypeSize else { return metrics.scaledValue(for: value) }
        let traits = UITraitCollection(preferredContentSizeCategory: UIContentSizeCategory(dynamicTypeSize))
        return metrics.scaledValue(for: value, compatibleWith: traits)
        #else
        return value
        #endif
    }

    /// The `wght` value a face takes under Bold Text.
    static func bolder(_ weight: CGFloat) -> CGFloat { min(900, weight + 180) }

    /// Bold Text read where UIKit allows it, on the main thread; elsewhere the regular weight.
    /// `View.o7Font(_:)` reads it from the environment instead.
    private static func boldened(_ weight: CGFloat) -> CGFloat {
        #if canImport(UIKit)
        let bold = Thread.isMainThread && MainActor.assumeIsolated { UIAccessibility.isBoldTextEnabled }
        return bold ? bolder(weight) : weight
        #else
        return weight
        #endif
    }

    private struct Key: Hashable {
        let size: CGFloat
        let weight: CGFloat
        let mono: Bool
        let tabular: Bool
    }

    private static let cache = Mutex<[Key: Font]>([:])

    /// A face at any `wght` value, made once per size and weight.
    static func font(size: CGFloat, weight: CGFloat, mono: Bool, tabular: Bool) -> Font {
        let key = Key(size: (size * 100).rounded() / 100, weight: weight, mono: mono, tabular: tabular)
        if let font = cache.withLock({ $0[key] }) { return font }
        let font = Font(ctFont(size: key.size, weight: weight, mono: mono, tabular: tabular))
        cache.withLock { $0[key] = font }
        return font
    }

    /// The face as a CoreText font, its weight set on the `wght` axis.
    static func ctFont(size: CGFloat, weight: CGFloat, mono: Bool, tabular: Bool) -> CTFont {
        O7Fonts.register()
        var attributes: [CFString: Any] = [
            kCTFontNameAttribute: mono ? O7Fonts.monoPostScriptName : O7Fonts.sansPostScriptName,
            kCTFontVariationAttribute: [wghtAxis: weight],
        ]
        if tabular {
            attributes[kCTFontFeatureSettingsAttribute] = [
                [kCTFontOpenTypeFeatureTag: "tnum", kCTFontOpenTypeFeatureValue: 1]
            ]
        }
        let descriptor = CTFontDescriptorCreateWithAttributes(attributes as CFDictionary)
        return CTFontCreateWithFontDescriptor(descriptor, size, nil)
    }

    /// The `wght` axis tag as CoreText identifies it.
    static let wghtAxis = 0x7767_6874  // 'wght'

    #if canImport(UIKit)
    private static func uiTextStyle(_ style: Font.TextStyle) -> UIFont.TextStyle {
        switch style {
        case .largeTitle: .largeTitle
        case .title: .title1
        case .title2: .title2
        case .title3: .title3
        case .headline: .headline
        case .subheadline: .subheadline
        case .body: .body
        case .callout: .callout
        case .footnote: .footnote
        case .caption: .caption1
        case .caption2: .caption2
        default: .body
        }
    }
    #endif
}

extension View {
    /// Sets an o7 text style: its font at the Dynamic Type size in effect (following changes),
    /// heavier under Bold Text, with the style's line height and tracking scaled alike.
    public func o7Font(_ style: O7TextStyle) -> some View {
        modifier(O7FontModifier(style: style))
    }
}

private struct O7FontModifier: ViewModifier {
    let style: O7TextStyle
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @Environment(\.legibilityWeight) private var legibilityWeight

    func body(content: Content) -> some View {
        let size = O7Font.scaled(style.size, relativeTo: style.relativeTo, dynamicTypeSize: dynamicTypeSize)
        let lineHeight = O7Font.scaled(style.lineHeight, relativeTo: style.relativeTo, dynamicTypeSize: dynamicTypeSize)
        content
            .font(
                O7Font.dynamic(
                    size: style.size, weight: style.weight, mono: style.mono, tabular: style.tabular,
                    relativeTo: style.relativeTo, bold: legibilityWeight == .bold))
            .lineHeight(.exact(points: lineHeight))
            .tracking(style.tracking * size)
    }
}
