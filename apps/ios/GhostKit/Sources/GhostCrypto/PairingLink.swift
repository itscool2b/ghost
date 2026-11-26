public import Foundation

/// A pairing link, as the computer's Settings shows it in a code:
/// `https://onghost.dev/remote#<computer id>.<pairing id>.<key>`, with 16, 8 and 32 random bytes in
/// base64url. The key is after the `#`, so it never reaches a server.
public struct PairingLink: Sendable, Hashable {
    /// The site the link was made for, such as `https://onghost.dev`: its relay is there.
    public let site: URL
    public let hostId: String
    public let pairingId: String
    public let key: Data

    public init(site: URL, hostId: String, pairingId: String, key: Data) {
        self.site = site
        self.hostId = hostId
        self.pairingId = pairingId
        self.key = key
    }

    /// The sites a release build pairs with. A link from anywhere else is refused, so a lookalike
    /// code cannot point the app at someone else's relay.
    public static let releaseSites = ["https://onghost.dev"]

    /// Development builds also take the preview site and a site on this computer.
    public static let developmentSites = [
        "https://onghost.dev", "https://preview.onghost.dev", "http://localhost", "http://127.0.0.1",
    ]

    /// Reads a scanned or pasted link. `sites` are the origins it may come from; a development
    /// origin without a port matches any port.
    public static func parse(_ text: String, sites: [String] = releaseSites) -> PairingLink? {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard let url = URL(string: trimmed), let scheme = url.scheme?.lowercased(),
            let host = url.host?.lowercased(), url.user == nil, url.password == nil,
            url.path == "/remote" || url.path == "/remote/",
            let fragment = url.fragment
        else { return nil }
        let origin = "\(scheme)://\(host)"
        let withPort = url.port.map { "\(origin):\($0)" } ?? origin
        guard sites.contains(where: { $0 == withPort || ($0 == origin && $0.hasPrefix("http://")) })
        else { return nil }
        guard let parts = parseFragment(fragment), let site = URL(string: withPort) else { return nil }
        return PairingLink(site: site, hostId: parts.0, pairingId: parts.1, key: parts.2)
    }

    /// The three parts after the `#`, each in its canonical spelling and length.
    public static func parseFragment(_ fragment: String) -> (String, String, Data)? {
        let parts = fragment.split(separator: ".", omittingEmptySubsequences: false).map(String.init)
        guard parts.count == 3, Base64URL.decode(parts[0], length: 16) != nil,
            Base64URL.decode(parts[1], length: 8) != nil,
            let key = Base64URL.decode(parts[2], length: 32)
        else { return nil }
        return (parts[0], parts[1], key)
    }
}
