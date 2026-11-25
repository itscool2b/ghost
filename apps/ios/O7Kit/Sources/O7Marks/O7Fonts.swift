import CoreText
import Foundation
import Synchronization
import os

/// Geist and Geist Mono, the variable fonts o7 sets its interface and code in, bundled in
/// `Fonts/` (vercel/geist-font v1.9.2, SIL Open Font License 1.1, `Fonts/OFL.txt`).
public enum O7Fonts {
    /// The interface face's family name.
    public static let sansFamily = "Geist"
    /// The code face's family name.
    public static let monoFamily = "Geist Mono"
    /// PostScript names of the faces' regular instances, which take any weight on their `wght`
    /// axis (100 to 900).
    public static let sansPostScriptName = "Geist-Regular"
    public static let monoPostScriptName = "GeistMono-Regular"

    static let fileNames = ["Geist[wght].ttf", "GeistMono[wght].ttf"]

    private static let registered = Mutex<Bool?>(nil)
    private static let log = Logger(subsystem: "codes.o7.mobile", category: "fonts")

    /// Registers both faces for this process with `CTFontManagerRegisterFontURLs`. Safe to call
    /// any number of times, from any thread: only the first call registers. Returns whether both
    /// faces are available.
    @discardableResult
    public static func register() -> Bool {
        registered.withLock { state in
            if let state { return state }
            let urls = fontURLs
            if !urls.isEmpty {
                CTFontManagerRegisterFontURLs(urls as CFArray, .process, true) { errors, _ in
                    // A face some other code registered first is already usable.
                    for error in (errors as? [CFError]) ?? [] where CFErrorGetCode(error) != CTFontManagerError.alreadyRegistered.rawValue {
                        log.error("Could not register a font: \(String(describing: error), privacy: .public)")
                    }
                    return true
                }
            }
            let available = isAvailable(sansPostScriptName, family: sansFamily)
                && isAvailable(monoPostScriptName, family: monoFamily)
            state = available
            return available
        }
    }

    /// The bundled font files.
    static var fontURLs: [URL] {
        fileNames.compactMap { name in
            let dot = name.lastIndex(of: ".") ?? name.endIndex
            return Bundle.module.url(
                forResource: String(name[..<dot]), withExtension: "ttf", subdirectory: "Fonts"
            )
        }
    }

    /// Whether the name finds the face itself, not CoreText's fallback for an unknown name.
    private static func isAvailable(_ postScriptName: String, family: String) -> Bool {
        let font = CTFontCreateWithName(postScriptName as CFString, 12, nil)
        return CTFontCopyFamilyName(font) as String == family
    }
}
