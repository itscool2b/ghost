public import Foundation

/// One WebSocket to the relay. Text frames only: the relay never sends binary.
public protocol WebSocketLink: Sendable {
    func send(_ text: String) async throws
    /// The next message; throws when the socket closed.
    func receive() async throws -> String
    func close()
}

/// Opens WebSockets. Tests stand in a transport that talks to a computer in a box.
public protocol WebSocketTransport: Sendable {
    func open(_ url: URL) async throws -> any WebSocketLink
}

public enum TransportError: Error, Sendable {
    case binary
    case closed
}

/// The system's WebSocket. It sends no `Origin` header, which is what the relay's app path asks
/// of a native client: no web page can open a socket there.
public final class URLSessionTransport: WebSocketTransport {
    public init() {}

    public func open(_ url: URL) async throws -> any WebSocketLink {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.waitsForConnectivity = false
        configuration.tlsMinimumSupportedProtocolVersion = .TLSv12
        let session = URLSession(configuration: configuration)
        let task = session.webSocketTask(with: url)
        // The computer may send up to 2 MiB in one message (a history page, a picture).
        task.maximumMessageSize = 4 * 1024 * 1024
        task.resume()
        return Link(session: session, task: task)
    }

    final class Link: WebSocketLink {
        let session: URLSession
        let task: URLSessionWebSocketTask

        init(session: URLSession, task: URLSessionWebSocketTask) {
            self.session = session
            self.task = task
        }

        func send(_ text: String) async throws {
            try await task.send(.string(text))
        }

        func receive() async throws -> String {
            switch try await task.receive() {
            case .string(let text): return text
            case .data: throw TransportError.binary
            @unknown default: throw TransportError.binary
            }
        }

        func close() {
            task.cancel(with: .normalClosure, reason: nil)
            session.invalidateAndCancel()
        }
    }
}

/// Where a computer's relay is for a site: `wss://onghost.dev/api/remote/v1/app/<computer id>`.
public struct RelayEndpoint: Sendable, Hashable {
    /// The relay's WebSocket origin, such as `wss://onghost.dev`.
    public let origin: URL

    public init(origin: URL) {
        self.origin = origin
    }

    /// The relay of a site: the same host, over `wss` (or `ws` for a development site).
    public init?(site: URL) {
        guard let host = site.host, let scheme = site.scheme else { return nil }
        let socket = scheme == "https" ? "wss" : "ws"
        let port = site.port.map { ":\($0)" } ?? ""
        guard let origin = URL(string: "\(socket)://\(host)\(port)") else { return nil }
        self.origin = origin
    }

    public func url(hostId: String) -> URL {
        origin.appendingPathComponent("api/remote/v1/app").appendingPathComponent(hostId)
    }
}
