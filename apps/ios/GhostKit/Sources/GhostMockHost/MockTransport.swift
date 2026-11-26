public import Foundation
public import GhostClient

/// Opens sockets to a `MockComputer` instead of the relay. Every socket is a fresh connection, as
/// the relay gives one, so reconnects, wakes and refusals behave as they would for real.
public final class MockTransport: WebSocketTransport {
    public let computer: MockComputer
    private let lock = NSLock()
    nonisolated(unsafe) private var links: [MockLink] = []

    public init(computer: MockComputer) {
        self.computer = computer
    }

    public func open(_ url: URL) async throws -> any WebSocketLink {
        let link = MockLink(computer: computer)
        lock.withLock { links.append(link) }
        await link.attach()
        return link
    }

    /// Silences every open socket, as a phone's socket goes quiet after it slept.
    public func silenceAll() {
        lock.withLock { for link in links { link.silenced = true } }
    }
}

/// One socket to the mock computer.
public final class MockLink: WebSocketLink, @unchecked Sendable {
    private let computer: MockComputer
    private let lock = NSLock()
    private var inbox: [String] = []
    private var waiting: CheckedContinuation<String, any Error>?
    private var closed = false
    private var session = 0
    var silenced = false

    init(computer: MockComputer) {
        self.computer = computer
    }

    func attach() async {
        session = await computer.attach(self)
    }

    /// The computer (or the relay) sends this socket a message.
    func deliver(_ text: String) {
        let next: CheckedContinuation<String, any Error>? = lock.withLock {
            guard !closed, !silenced else { return nil }
            if let waiting {
                self.waiting = nil
                return waiting
            }
            inbox.append(text)
            return nil
        }
        next?.resume(returning: text)
    }

    func closeFromServer() {
        close()
    }

    public func send(_ text: String) async throws {
        let (gone, quiet) = lock.withLock { (closed, silenced) }
        if gone { throw TransportError.closed }
        if quiet { return }
        await computer.received(text, session: session)
    }

    public func receive() async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            let ready: Result<String, any Error>? = lock.withLock {
                if closed { return .failure(TransportError.closed) }
                if !inbox.isEmpty { return .success(inbox.removeFirst()) }
                waiting = continuation
                return nil
            }
            if let ready { continuation.resume(with: ready) }
        }
    }

    public func close() {
        let pending: CheckedContinuation<String, any Error>? = lock.withLock {
            guard !closed else { return nil }
            closed = true
            defer { waiting = nil }
            return waiting
        }
        pending?.resume(throwing: TransportError.closed)
        let id = session
        Task { [computer] in await computer.detach(id) }
    }
}
