import Foundation
import GhostClient
import GhostCrypto
import GhostMockHost
import GhostPush
import GhostStore

/// Where the app's computers are: the real relay, or a computer in a box for UI tests, previews
/// and screenshots (`-GhostBackend mock`).
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
        if arguments.contains("-GhostBackend"), defaults.string(forKey: "GhostBackend") == "mock" {
            let computer = MockComputer(hostName: "Studio", hostId: MockComputer.demoHostId)
            // The App Group's folder, so the notification extension reads the demo's name and
            // device as it would a real computer's. Its key stays in memory.
            return Backend(
                transport: MockTransport(computer: computer),
                computers: Computers(folder: Computers.defaultFolder(), secrets: MemorySecretStore()),
                sites: PairingLink.developmentSites, relayOverride: nil, mock: computer)
        }
        #if GHOST_DEV_TOOLS
            let sites = PairingLink.developmentSites
            let relay = defaults.string(forKey: "GhostRelay").flatMap(URL.init(string:))
        #else
            let sites = PairingLink.releaseSites
            let relay: URL? = nil
        #endif
        let secrets = KeychainSecretStore()
        // The Keychain keeps items across a reinstall; a fresh install starts with no computer.
        if !defaults.bool(forKey: "installed") {
            secrets.deleteAll()
            KeychainPushKeys().deleteAll()
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
