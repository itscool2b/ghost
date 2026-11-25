import Foundation
import O7Wire
import Testing

@testable import O7Transcript

// Every golden written by apps/ios/scripts/write-goldens.mts from the desktop's TypeScript: the port
// must give the same JSON, value for value. A failure names the first path that differs.

@Suite("Goldens from the desktop's TypeScript")
struct GoldenTests {
    @Test func goldensExist() {
        #expect(Golden.all.count > 300)
    }

    @Test(arguments: Golden.all)
    func golden(_ file: Golden) throws {
        let data = try Data(contentsOf: file.url)
        let head = try JSONDecoder().decode(Head.self, from: data)
        switch head.kind {
        case "transcript":
            let golden = try JSONDecoder().decode(TranscriptGolden.self, from: data)
            let checks = try runTranscript(golden.input.ops)
            #expect(checks.count == golden.expected.checks.count, "\(golden.name): number of checks")
            for (index, (actual, expected)) in zip(checks, golden.expected.checks).enumerated() {
                if let difference = firstDifference(actual, expected, at: "check[\(index)]") {
                    Issue.record("\(golden.name): \(difference)")
                }
            }
        case "calls":
            let golden = try JSONDecoder().decode(CallsGolden.self, from: data)
            var failures = 0
            for (index, entry) in golden.calls.enumerated() {
                do {
                    let actual = try call(entry.fn, entry.args)
                    if let difference = firstDifference(actual, entry.out, at: "calls[\(index)] \(entry.fn)") {
                        failures += 1
                        if failures <= 20 { Issue.record("\(golden.name): \(difference)") }
                    }
                } catch {
                    failures += 1
                    if failures <= 20 { Issue.record("\(golden.name): calls[\(index)] \(entry.fn) threw \(error)") }
                }
            }
            if failures > 20 { Issue.record("\(golden.name): \(failures - 20) more calls differ") }
        default:
            Issue.record("\(file.testDescription): unknown kind \(head.kind)")
        }
    }

    private struct Head: Decodable { let kind: String }
}
