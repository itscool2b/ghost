import Foundation
import GhostMarkdown
import Testing

@Test func readsWhatAgentsWrite() throws {
    let text = """
        # Done

        The submit handler never checks the **email** field, so an empty form posts.

        ```ts
        if (!email.includes("@")) return;
        ```

        - one
        - two with `code`

        1. first
        2. second

        > quoted

        | Case | Before | After |
        | --- | :---: | ---: |
        | Empty | Posted | Refused |

        ---

        [site](https://onghost.dev) and [not this](javascript:alert(1)) and ~~gone~~.
        """
    let blocks = Markdown.blocks(text)
    guard blocks.count == 9 else {
        Issue.record("got \(blocks.count) blocks: \(blocks)")
        return
    }
    guard case .heading(1, let title) = blocks[0] else { Issue.record("heading"); return }
    #expect(String(title.characters) == "Done")
    guard case .paragraph = blocks[1] else { Issue.record("paragraph"); return }
    guard case .code(let language, let code) = blocks[2] else { Issue.record("code"); return }
    #expect(language == "ts")
    #expect(code == #"if (!email.includes("@")) return;"#)
    guard case .list(false, _, let items) = blocks[3] else { Issue.record("list"); return }
    #expect(items.count == 2)
    guard case .list(true, 1, let numbered) = blocks[4] else { Issue.record("numbered"); return }
    #expect(numbered.count == 2)
    guard case .quote(let inner) = blocks[5], case .paragraph = inner.first else { Issue.record("quote"); return }
    guard case .table(let header, let rows, let alignments) = blocks[6] else { Issue.record("table"); return }
    #expect(header.map { String($0.characters) } == ["Case", "Before", "After"])
    #expect(rows.count == 1)
    #expect(alignments == [.leading, .center, .trailing])
    guard case .rule = blocks[7] else { Issue.record("rule"); return }
    guard case .paragraph(let last) = blocks[8] else { Issue.record("links"); return }
    let links = last.runs.compactMap(\.link)
    #expect(links == [URL(string: "https://onghost.dev")!])
}

@Test func plainTextStaysAsItIs() {
    let blocks = Markdown.blocks("Just words.")
    guard case .paragraph(let text) = blocks.first else { Issue.record("paragraph"); return }
    #expect(String(text.characters) == "Just words.")
}
