import CryptoKit
public import Foundation
public import GhostWire

extension HostConnection {
    /// Sends `data` to the computer a chunk at a time, for a message to carry: each chunk waits for
    /// the last one's answer, which keeps a photo inside the relay's limits (96 KiB a message, 240
    /// messages in ten seconds). Returns the upload's id. `progress` hears 0 to 1.
    public func upload(
        _ data: Data, name: String, mime: String, progress: (@Sendable (Double) -> Void)? = nil
    ) async throws -> String {
        let sha = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        let begun = try await send(
            .uploadBegin(name: name, mime: mime, size: data.count, sha256: sha), as: UploadBegun.self)
        let chunk = max(1024, min(begun.chunk, 49_152))
        var index = 0
        var offset = 0
        do {
            while offset < data.count {
                let end = min(offset + chunk, data.count)
                let piece = data.subdata(in: offset..<end)
                let answer = try await send(
                    .uploadChunk(begun.uploadId, index: index, data: piece), as: UploadProgress.self)
                offset = end
                index += 1
                progress?(Double(offset) / Double(max(1, data.count)))
                if answer.complete { break }
            }
        } catch {
            try? await perform(.uploadAbort(begun.uploadId))
            throw error
        }
        return begun.uploadId
    }
}
