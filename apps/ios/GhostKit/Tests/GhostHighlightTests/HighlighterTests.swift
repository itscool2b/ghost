import Foundation
import GhostTheme
import SwiftUI
import Testing

@testable import GhostHighlight

/// What `node apps/ios/scripts/write-highlighter.mjs` read from Node running the same bundle.
private struct Goldens: Decodable {
    struct Run: Decodable {
        let start: Int
        let end: Int
        let scopes: [String]
    }

    struct Sample: Decodable {
        let fence: String
        let language: String
        let code: String
        let html: String
        let runs: [Run]

        var highlightRuns: [HighlightRun] {
            runs.map { HighlightRun(range: $0.start..<$0.end, scopes: $0.scopes) }
        }
    }

    let version: String
    let maxChars: Int
    let samples: [Sample]

    static func load() throws -> Goldens {
        try JSONDecoder().decode(Goldens.self, from: Data(HighlightGoldens.json.utf8))
    }
}

private func plain(_ code: String) -> [HighlightRun] {
    [HighlightRun(range: 0..<code.utf16.count, scopes: [])]
}

@Suite("Highlighter")
struct HighlighterTests {
    @Test("writes the same HTML in JavaScriptCore as Node does with the same bundle")
    func html() async throws {
        let goldens = try Goldens.load()
        #expect(goldens.samples.map(\.fence) == ["swift", "ts", "rust", "python", "sh", "json", "diff"])
        let highlighter = Highlighter()
        for sample in goldens.samples {
            let html = await highlighter.html(code: sample.code, fence: sample.fence)
            #expect(html == sample.html, "\(sample.fence)")
        }
    }

    @Test("reads the same runs from that HTML as the generator's parser")
    func parser() throws {
        for sample in try Goldens.load().samples {
            let parsed = try #require(HighlightHTML.parse(sample.html), "\(sample.fence)")
            #expect(parsed.text == Array(sample.code.utf16), "\(sample.fence)")
            #expect(parsed.runs == sample.highlightRuns, "\(sample.fence)")
        }
    }

    @Test("colours each sample end to end")
    func tokens() async throws {
        let highlighter = Highlighter()
        for sample in try Goldens.load().samples {
            let runs = await highlighter.tokens(code: sample.code, language: sample.fence)
            #expect(runs == sample.highlightRuns, "\(sample.fence)")
            #expect(runs.contains { !$0.scopes.isEmpty }, "\(sample.fence)")
        }
    }

    @Test("resolves the desktop's aliases")
    func aliases() async throws {
        let highlighter = Highlighter()
        let goldens = try Goldens.load()
        for sample in goldens.samples {
            #expect(await highlighter.language(for: sample.fence) == sample.language)
        }
        #expect(await highlighter.language(for: "sh") == "bash")
        #expect(await highlighter.language(for: "zsh") == "bash")
        #expect(await highlighter.language(for: " TSX ") == "typescript")
        #expect(await highlighter.language(for: "yml") == "yaml")
        #expect(await highlighter.language(for: "cobol") == nil)
        let bash = try #require(goldens.samples.first { $0.fence == "sh" })
        let expected = bash.highlightRuns
        for fence in ["sh", "zsh", "bash", "shell", "console"] {
            #expect(await highlighter.tokens(code: bash.code, language: fence) == expected, "\(fence)")
        }
    }

    @Test("keeps code plain without a language it knows")
    func unknown() async {
        let highlighter = Highlighter()
        let code = "let x = 1 // é"
        #expect(await highlighter.tokens(code: code, language: "cobol") == plain(code))
        #expect(await highlighter.tokens(code: code, language: nil) == plain(code))
        #expect(await highlighter.tokens(code: code, language: "  ") == plain(code))
        #expect(await highlighter.tokens(code: "", language: "swift") == [])
    }

    @Test("leaves code over the desktop's limit plain, counting UTF-16 as JavaScript does")
    func limit() async throws {
        let highlighter = Highlighter()
        #expect(Highlighter.maxCharacters == 100_000)
        let constants = try #require(await highlighter.bundleConstants())
        #expect(constants.version == "11.12.0")
        #expect(constants.maxChars == Highlighter.maxCharacters)
        #expect(try Goldens.load().maxChars == Highlighter.maxCharacters)

        let over = String(repeating: "x", count: 100_001)
        #expect(await highlighter.tokens(code: over, language: "swift") == plain(over))
        // 50,001 characters, but 100,002 UTF-16 code units.
        let wide = String(repeating: "\u{1F642}", count: 50_001)
        #expect(await highlighter.tokens(code: wide, language: "json") == plain(wide))

        let line = "echo hi # note\n"
        var atLimit = String(repeating: line, count: 100_000 / line.utf16.count)
        atLimit += String(repeating: " ", count: 100_000 - atLimit.utf16.count)
        #expect(atLimit.utf16.count == 100_000)
        let runs = await highlighter.tokens(code: atLimit, language: "bash")
        #expect(runs.count > 1)
        #expect(runs.last?.range.upperBound == 100_000)
    }

    @Test("caches results and makes the context again after letting it go")
    func cache() async throws {
        let highlighter = Highlighter()
        let sample = try #require(try Goldens.load().samples.first)
        let first = await highlighter.tokens(code: sample.code, language: sample.fence)
        let second = await highlighter.tokens(code: sample.code, language: sample.fence)
        #expect(first == second)
        await highlighter.releaseResources()
        #expect(await highlighter.tokens(code: sample.code, language: sample.fence) == first)
        #expect(await Highlighter.shared.tokens(code: sample.code, language: sample.fence) == first)
    }
}

@Suite("highlight.js HTML")
struct HighlightHTMLTests {
    @Test("decodes the entities highlight.js writes, and numeric ones")
    func entities() throws {
        let html = "a &amp; b &lt;c&gt; &quot;d&quot; &#x27;e&#39; &#169; &#xE9; &#X1F642;"
        let parsed = try #require(HighlightHTML.parse(html))
        let text = "a & b <c> \"d\" 'e' \u{A9} \u{E9} \u{1F642}"
        #expect(parsed.text == Array(text.utf16))
        #expect(parsed.runs == plain(text))
    }

    @Test("refuses any other markup")
    func strict() {
        for html in [
            "<b>x</b>",
            #"<span class="hljs-string" onclick="x">a</span>"#,
            "<script>alert(1)</script>",
            "<SPAN class=\"hljs-string\">x</SPAN>",
            "<span class='hljs-string'>x</span>",
            #"<span class="hljs-string"><img src=x></span>"#,
            #"<span class="hljs-string">open"#,
            "</span>",
            "a < b",
            #"<span class="">x</span>"#,
            #"<span class="evil">x</span>"#,
            #"<span class="hljs-title class">x</span>"#,
            #"<span class="hljs-string" >x</span>"#,
            #"<span class="hljs-string;x">x</span>"#,
            "a &nbsp; b",
            "a & b",
            "&#xD800;",
            "&#x110000;",
            "&#;",
            "&;",
        ] {
            #expect(HighlightHTML.parse(html) == nil, "\(html)")
        }
    }

    @Test("names scopes as highlight.js does")
    func scopes() throws {
        let html = #"<span class="hljs-title class_ inherited__">A</span><span class="language-xml"><span class="hljs-tag">b</span></span>c"#
        let parsed = try #require(HighlightHTML.parse(html))
        #expect(parsed.runs == [
            HighlightRun(range: 0..<1, scopes: ["title.class.inherited"]),
            HighlightRun(range: 1..<2, scopes: ["language:xml", "tag"]),
            HighlightRun(range: 2..<3, scopes: []),
        ])
        #expect(parsed.runs[1].scope == "tag")
        #expect(HighlightHTML.scope(ofClass: "hljs-built_in") == "built_in")
        #expect(HighlightHTML.scope(ofClass: "hljs-title function_ invoke__") == "title.function.invoke")
        #expect(HighlightHTML.scope(ofClass: "hljs-title function__") == "title.function_")
        #expect(HighlightHTML.scope(ofClass: "hljs-") == nil)
        #expect(HighlightHTML.scope(ofClass: "language-") == nil)
    }

    @Test("merges neighbours with the same scopes and drops empty spans")
    func merging() throws {
        let html = #"<span class="hljs-string">a</span><span class="hljs-string">b</span><span class="hljs-number"></span>c"#
        let parsed = try #require(HighlightHTML.parse(html))
        #expect(parsed.runs == [
            HighlightRun(range: 0..<2, scopes: ["string"]),
            HighlightRun(range: 2..<3, scopes: []),
        ])
    }

    @Test("never splits a surrogate pair between runs")
    func surrogates() throws {
        let units = Array(#"<span class="hljs-string">"#.utf16) + [0xD83D] + Array("</span>".utf16) + [0xDE42, 0x61]
        let parsed = try #require(HighlightHTML.parse(units: units))
        #expect(parsed.text == [0xD83D, 0xDE42, 0x61])
        #expect(parsed.runs == [
            HighlightRun(range: 0..<2, scopes: ["string"]),
            HighlightRun(range: 2..<3, scopes: []),
        ])
    }
}

@Suite("Code colours")
struct CodeColourTests {
    @Test("maps scopes to the desktop stylesheet's colours")
    func roles() {
        #expect(HighlightRole(scopes: ["meta", "keyword"]) == .keyword)
        #expect(HighlightRole(scopes: ["string", "subst"]) == .string)
        #expect(HighlightRole(scopes: ["string", "subst", "variable"]) == .attr)
        #expect(HighlightRole(scopes: ["title.class"]) == .title)
        #expect(HighlightRole(scopes: ["title.function.invoke"]) == .title)
        #expect(HighlightRole(scopes: ["language:xml"]) == .plain)
        #expect(HighlightRole(scopes: ["operator"]) == .plain)
        #expect(HighlightRole(scopes: []) == .plain)
        #expect(HighlightRole(scopes: ["type"]) == .keyword)
        #expect(HighlightRole(scopes: ["built_in"]) == .keyword)
        #expect(HighlightRole(scopes: ["selector-pseudo"]) == .string)
        #expect(HighlightRole(scopes: ["bullet"]) == .number)
        #expect(HighlightRole(scopes: ["params"]) == .attr)
        #expect(HighlightRole(scopes: ["quote"]) == .comment)
        #expect(HighlightRole(scopes: ["addition"]) == .addition)
        #expect(HighlightRole(scopes: ["deletion"]) == .deletion)
        let emphasis = HighlightRun(range: 0..<1, scopes: ["section", "emphasis"])
        #expect(emphasis.role == .title)
        #expect(emphasis.isEmphasized)
        #expect(!emphasis.isStrong)
        #expect(HighlightRun(range: 0..<1, scopes: ["strong", "string"]).isStrong)

        let palette = Palettes.palette(.claude, dark: true)
        #expect(HighlightRole.plain.color(in: palette) == palette.codeText)
        #expect(HighlightRole.keyword.color(in: palette) == palette.syntaxKeyword)
        #expect(HighlightRole.string.color(in: palette) == palette.syntaxString)
        #expect(HighlightRole.number.color(in: palette) == palette.syntaxNumber)
        #expect(HighlightRole.title.color(in: palette) == palette.syntaxTitle)
        #expect(HighlightRole.attr.color(in: palette) == palette.syntaxAttr)
        #expect(HighlightRole.comment.color(in: palette) == palette.syntaxComment)
        #expect(HighlightRole.addition.color(in: palette) == palette.syntaxAddition)
        #expect(HighlightRole.deletion.color(in: palette) == palette.syntaxDeletion)
    }

    @Test("builds an AttributedString over the whole code in the palette's colours")
    func attributed() throws {
        let sample = try #require(try Goldens.load().samples.first { $0.fence == "swift" })
        let palette = Palettes.palette(.codex, dark: false)
        let text = Highlighter.attributedString(sample.code, runs: sample.highlightRuns, palette: palette)
        #expect(String(text.characters) == sample.code)

        func color(of word: String) throws -> Color? {
            let range = try #require(text.range(of: word))
            return text[range].runs.first?.foregroundColor
        }
        #expect(try color(of: "struct") == Color(palette.syntaxKeyword))
        #expect(try color(of: "Foundation") == Color(palette.codeText))
        #expect(try color(of: "/// Greets") == Color(palette.syntaxComment))
        #expect(try color(of: "Greeter") == Color(palette.syntaxTitle))
        #expect(try color(of: "0x1F") == Color(palette.syntaxNumber))
    }

    @Test("sets emphasis and strong, and fills gaps with plain code")
    func intents() {
        let palette = Palettes.palette(.aura, dark: true)
        let runs = [
            HighlightRun(range: 1..<3, scopes: ["emphasis"]),
            HighlightRun(range: 3..<5, scopes: ["strong", "string"]),
            HighlightRun(range: 4..<9, scopes: ["keyword"]),
        ]
        let text = Highlighter.attributedString("abcdefg", runs: runs, palette: palette)
        #expect(String(text.characters) == "abcdefg")
        let pieces = text.runs.map { (String(text[$0.range].characters), $0.foregroundColor, $0.inlinePresentationIntent) }
        #expect(pieces.map(\.0) == ["a", "bc", "de", "fg"])
        #expect(pieces.map(\.1) == [
            Color(palette.codeText), Color(palette.codeText), Color(palette.syntaxString), Color(palette.syntaxKeyword),
        ])
        #expect(pieces.map(\.2) == [nil, .emphasized, .stronglyEmphasized, nil])
    }
}
