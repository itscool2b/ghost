import Foundation
import Observation
import O7Client
import O7Crypto
import O7MockHost
import O7Push
import O7Store
import O7Wire
import SwiftUI

/// The app's state: the paired computers, the one in view, a pairing in progress, and the person's
/// own settings.
@Observable
final class AppModel {
    let backend: Backend
    private(set) var computers: [ComputerStore] = []
    /// The computer the tabs show.
    var selectedHostId: String?
    /// A pairing link waiting for the person's yes (scanned, pasted or opened as a link).
    var proposed: PairingLink?
    /// The pairing under way, after the yes.
    private(set) var pairing: ComputerStore?
    /// The scanner is showing.
    var scanning = false
    /// A problem to say once, such as a code that is not o7's.
    var notice: String?
    let settings = Settings()
    /// Notifications: the person's yes, Apple's token, what each computer should send.
    let push = PushHub()
    @ObservationIgnored let activities = LiveActivities()
    /// A chat to open, from a notification or a link; the tabs take it.
    var route: ChatRoute?
    /// The chat on screen, so its notifications do not show a banner.
    var visibleChat: ChatRoute?
    @ObservationIgnored var snapshotWrite: Task<Void, Never>?
    @ObservationIgnored var lastSnapshot: SharedSnapshot?

    init(backend: Backend = .current()) {
        self.backend = backend
        for record in backend.computers.all() {
            guard let credential = backend.computers.credential(for: record.hostId) else { continue }
            computers.append(ComputerStore(record: record, credential: credential, backend: backend))
        }
        selectedHostId = computers.first?.hostId
        for computer in computers { connect(computer) }
        // Here and not in `start()`: a notification's button can launch the app in the background,
        // before any screen appears.
        NotificationRouter.shared.attach(self)
    }

    /// Hands a computer what the app keeps for all of them.
    private func connect(_ computer: ComputerStore) {
        computer.push = push
        computer.changed = { [weak self] in self?.scheduleSnapshot() }
    }

    var selected: ComputerStore? {
        computers.first { $0.hostId == selectedHostId } ?? computers.first
    }

    /// The look the app wears: the person's choice, else the computer in view's.
    var look: Look {
        settings.themeOverride.map { Look(theme: $0, appearance: settings.appearance ?? "system") }
            ?? selected.map { computer in
                Look(theme: computer.look.theme, appearance: settings.appearance ?? computer.look.appearance)
            } ?? Look(theme: "claude", appearance: settings.appearance ?? "system")
    }

    // MARK: Life

    func start() {
        Task { await push.refreshStatus() }
        for computer in computers { computer.start() }
        // The demo computer, already paired, for screenshots and UI tests.
        if let mock = backend.mock, computers.isEmpty,
            ProcessInfo.processInfo.arguments.contains("-O7MockPaired")
        {
            Task { await pairedDemo(mock) }
        }
    }

    /// A pairing link from the demo computer, asked about as a scanned one would be.
    func pairDemo() async {
        guard let mock = backend.mock else { return }
        proposed = await mock.pairingLink()
    }

    private func pairedDemo(_ mock: MockComputer) async {
        // Fixed in the demo, so `scripts/push.mjs` can seal a notification for it.
        let device = await mock.addDevice(id: MockComputer.demoDeviceId, key: MockComputer.demoDeviceKey)
        let credential = DeviceCredential(hostId: mock.hostId, deviceId: device.id, key: device.key)
        _ = try? backend.computers.paired(
            credential, hostName: mock.hostName, site: "https://o7.codes",
            look: Look(theme: "claude", appearance: "dark"), features: ["pickers", "inbox", "search"])
        guard let record = backend.computers.all().first else { return }
        let store = ComputerStore(record: record, credential: credential, backend: backend)
        connect(store)
        computers = [store]
        selectedHostId = store.hostId
        store.start()
    }

    /// Back in front, or the network came back: every session checks itself.
    func wakeAll() {
        for computer in computers { computer.wake() }
    }

    // MARK: Pairing

    /// The link being paired came from the clipboard: once paired, the app offers to clear it.
    var pastedLink = false

    /// A scanned, pasted or opened link: asks first, whatever is on screen.
    func propose(_ text: String, pasted: Bool = false) {
        pastedLink = pasted
        guard let link = PairingLink.parse(text, sites: backend.sites) else {
            notice = "That is not an o7 pairing code. In o7 on your computer, open Settings, Remote access, Pair a device."
            return
        }
        scanning = false
        proposed = link
    }

    /// Whether this phone is already paired with the computer a link names.
    func known(_ link: PairingLink) -> ComputerStore? {
        computers.first { $0.hostId == link.hostId }
    }

    /// The person said yes.
    func pair(_ link: PairingLink, deviceName: String) {
        pairing?.stop()
        let store = ComputerStore(link: link, deviceName: deviceName, backend: backend)
        pairing = store
        store.start()
    }

    /// The pairing finished: the computer joins the list and comes into view, replacing an older
    /// pairing with the same computer.
    func adopt(_ store: ComputerStore) {
        guard store.isPaired else { return }
        if let old = computers.firstIndex(where: { $0.hostId == store.hostId }) {
            computers[old].stop()
            computers.remove(at: old)
        }
        connect(store)
        computers.insert(store, at: 0)
        selectedHostId = store.hostId
        pairing = nil
        proposed = nil
        Task { await store.preparePush() }
    }

    func cancelPairing() {
        if let pairing, !pairing.isPaired { pairing.stop() }
        pairing = nil
        proposed = nil
    }

    func select(_ hostId: String) {
        selectedHostId = hostId
        backend.computers.update(hostId) { $0.lastOpened = .now }
    }

    func forget(_ computer: ComputerStore) async {
        await computer.forget()
        computers.removeAll { $0.hostId == computer.hostId }
        if selectedHostId == computer.hostId { selectedHostId = computers.first?.hostId }
    }

    func discard(_ computer: ComputerStore) {
        computer.discard()
        computers.removeAll { $0.hostId == computer.hostId }
        if selectedHostId == computer.hostId { selectedHostId = computers.first?.hostId }
    }

    /// Everything that needs the person, across every computer.
    var needsYouCount: Int { computers.reduce(0) { $0 + $1.needsYou.count } }
    var workingCount: Int { computers.reduce(0) { $0 + $1.working.count } }
}

/// The person's own choices, kept on this phone.
@Observable
final class Settings {
    private let defaults = UserDefaults.standard

    /// A theme of the person's own instead of the computer's.
    var themeOverride: String? {
        didSet { defaults.set(themeOverride, forKey: "themeOverride") }
    }
    /// "system", "light" or "dark" instead of the computer's.
    var appearance: String? {
        didSet { defaults.set(appearance, forKey: "appearance") }
    }
    /// Ask for Face ID before allowing what an agent asks.
    var faceIdToApprove: Bool {
        didSet { defaults.set(faceIdToApprove, forKey: "faceIdToApprove") }
    }
    /// Ask for Face ID when the app opens.
    var lockWithFaceId: Bool {
        didSet { defaults.set(lockWithFaceId, forKey: "lockWithFaceId") }
    }

    init() {
        themeOverride = defaults.string(forKey: "themeOverride")
        appearance = defaults.string(forKey: "appearance")
        faceIdToApprove = defaults.bool(forKey: "faceIdToApprove")
        lockWithFaceId = defaults.bool(forKey: "lockWithFaceId")
    }
}
