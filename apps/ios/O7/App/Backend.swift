import Foundation
import O7Client
import O7Crypto
import O7MockHost
import O7Store

/// Where the app's computers are: the real relay, or a computer in a box for UI tests, previews
/// and screenshots (`-O7Backend mock`).
struct Backend: Sendable {
    let transport: any WebSocketTransport
    let computers: Computers
    let sites: [String]
    /// A relay of the developer's own, for every computer (development builds only).
    let relayOverride: URL?
    /// The mock computer, when the app runs against one.
    let mock: MockComputer?

    func endpoint(site: String) -> RelayEndpoint? {
        if let relayOverride { return RelayEndpoint(origin: relayOverride) }
        return URL(string: site).flatMap(RelayEndpoint.init(site:))
    }

    static func current(arguments: [String] = ProcessInfo.processInfo.arguments) -> Backend {
        let defaults = UserDefaults.standard
        if arguments.contains("-O7Backend"), defaults.string(forKey: "O7Backend") == "mock" {
            let computer = MockComputer(hostName: "Studio")
            let folder = FileManager.default.temporaryDirectory.appendingPathComponent(
                "o7-mock-\(ProcessInfo.processInfo.processIdentifier)")
            return Backend(
                transport: MockTransport(computer: computer),
                computers: Computers(folder: folder, secrets: MemorySecretStore()),
                sites: PairingLink.developmentSites, relayOverride: nil, mock: computer)
        }
        #if O7_DEV_TOOLS
            let sites = PairingLink.developmentSites
            let relay = defaults.string(forKey: "O7Relay").flatMap(URL.init(string:))
        #else
            let sites = PairingLink.releaseSites
            let relay: URL? = nil
        #endif
        let secrets = KeychainSecretStore()
        // The Keychain keeps items across a reinstall; a fresh install starts with no computer.
        if !defaults.bool(forKey: "installed") {
            secrets.deleteAll()
            defaults.set(true, forKey: "installed")
        }
        return Backend(
            transport: URLSessionTransport(),
            computers: Computers(folder: Computers.defaultFolder(), secrets: secrets),
            sites: sites, relayOverride: relay, mock: nil)
    }
}

/// This phone, as the computer's Settings will list it.
enum ThisDevice {
    static var model: String {
        var system = utsname()
        uname(&system)
        return withUnsafeBytes(of: &system.machine) { bytes in
            String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
        }
    }

    static var appVersion: String {
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "1.0"
    }

    static func client(name: String) -> ClientInfo {
        ClientInfo(name: name, version: appVersion, model: model)
    }
}
