import ActivityKit
import Foundation
import Observation
import GhostClient
import GhostPush
import GhostWire
import UIKit
import UserNotifications
import WidgetKit
import os

private let activityLog = Logger(subsystem: "dev.onghost.mobile", category: "activity")

/// Notifications on this phone: whether the person allowed them, where Apple delivers them, and
/// what the person wants from each computer.
@Observable
final class PushHub {
    /// Apple's token for this install, hex. Nil until registered.
    var token: String?
    var status: UNAuthorizationStatus = .notDetermined
    /// Why registering with Apple failed, said plainly.
    var failure: String?
    let keys: any PushKeyStore
    /// Apple's development or production service: the build decides.
    let environment: String
    private let defaults = UserDefaults.standard

    init(keys: any PushKeyStore = KeychainPushKeys()) {
        self.keys = keys
        #if DEBUG
            environment = "sandbox"
        #else
            environment = "production"
        #endif
    }

    var allowed: Bool { status == .authorized || status == .provisional || status == .ephemeral }

    func prefs(_ hostId: String) -> PushPrefs {
        guard let data = defaults.data(forKey: "push.prefs.\(hostId)"),
            let prefs = try? JSONDecoder().decode(PushPrefs.self, from: data)
        else { return .all }
        return prefs
    }

    func setPrefs(_ prefs: PushPrefs, for hostId: String) {
        defaults.set(try? JSONEncoder().encode(prefs), forKey: "push.prefs.\(hostId)")
    }

    /// Whether a Live Activity follows each computer's work on the Lock Screen.
    var liveActivities: Bool {
        get {
            access(keyPath: \.liveActivities)
            return defaults.object(forKey: "liveActivities") as? Bool ?? true
        }
        set {
            withMutation(keyPath: \.liveActivities) { defaults.set(newValue, forKey: "liveActivities") }
        }
    }

    func refreshStatus() async {
        status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
        if allowed { UIApplication.shared.registerForRemoteNotifications() }
    }

    /// Asks the person, once; afterwards only Settings can change it.
    func turnOn() async {
        let center = UNUserNotificationCenter.current()
        _ = try? await center.requestAuthorization(options: [.alert, .badge, .sound])
        await refreshStatus()
    }
}

/// The app's delegate, for what SwiftUI does not hand over: Apple's notification token.
final class AppDelegate: NSObject, UIApplicationDelegate {
    func application(
        _ application: UIApplication, didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = NotificationRouter.shared
        center.setNotificationCategories(NotificationWords.categories)
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
        NotificationRouter.shared.registered(token.map { String(format: "%02x", $0) }.joined())
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: any Error) {
        NotificationRouter.shared.model?.push.failure = error.localizedDescription
    }
}

/// What a notification's button answers.
enum NotificationReply: Sendable {
    case allow, deny, yes, no
    case text(String)
}

/// Where notifications land while the app runs, and what their buttons do.
final class NotificationRouter: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotificationRouter()
    weak var model: AppModel?
    /// A token that came before the model was there.
    private var early: String?

    func registered(_ token: String) {
        guard let model else {
            early = token
            return
        }
        model.pushRegistered(token)
    }

    func attach(_ model: AppModel) {
        self.model = model
        if let early {
            model.pushRegistered(early)
            self.early = nil
        }
    }

    /// No banner for the chat on screen: the person is looking at it. UIKit wants each answer on
    /// the main thread, hence the completion forms and not the async ones.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        let opened = PushOpener().open(notification.request.content.userInfo, remember: false)
        nonisolated(unsafe) let done = completionHandler
        Task { @MainActor in
            guard let opened, let chat = opened.payload.chat,
                self.model?.visibleChat == ChatRoute(hostId: opened.hostId, chatId: chat)
            else {
                done([.banner, .list, .sound, .badge])
                return
            }
            done([.list])
        }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        nonisolated(unsafe) let done = completionHandler
        guard let opened = PushOpener().open(response.notification.request.content.userInfo, remember: false) else {
            Task { @MainActor in done() }
            return
        }
        let reply: NotificationReply? =
            switch response.actionIdentifier {
            case PushCategory.Action.allow: .allow
            case PushCategory.Action.deny: .deny
            case PushCategory.Action.yes: .yes
            case PushCategory.Action.no: .no
            case PushCategory.Action.answer: (response as? UNTextInputNotificationResponse).map { .text($0.userText) }
            default: nil
            }
        Task { @MainActor in
            if let reply, let model = self.model {
                // The app stays up until the computer has the answer.
                await model.answer(opened, with: reply)
            } else {
                self.model?.show(opened)
            }
            done()
        }
    }
}

extension AppModel {
    func pushRegistered(_ token: String) {
        push.token = token
        push.failure = nil
        for computer in computers { Task { await computer.registerPush() } }
    }

    /// Opens the chat a notification is about.
    func show(_ opened: OpenedPush) {
        guard computers.contains(where: { $0.hostId == opened.hostId }) else { return }
        select(opened.hostId)
        if let chat = opened.payload.chat { route = ChatRoute(hostId: opened.hostId, chatId: chat) }
    }

    /// Answers what a notification asked, from its button, the app possibly not on screen. A
    /// failure is said in a notification of its own, so the person knows to open Ghost.
    func answer(_ opened: OpenedPush, with reply: NotificationReply) async {
        guard let computer = computers.first(where: { $0.hostId == opened.hostId }),
            let chat = opened.payload.chat, let request = opened.payload.request
        else { return }
        let background = UIApplication.shared.beginBackgroundTask(withName: "Ghost answer")
        defer { UIApplication.shared.endBackgroundTask(background) }
        let thread = "\(opened.hostId):\(chat)"
        do {
            guard await computer.ready(within: .seconds(12)) else { throw RemoteFailure.unreachable }
            switch reply {
            case .allow:
                try await computer.allow(chat, request: request.id, option: request.allow)
            case .deny:
                try await computer.allow(chat, request: request.id, option: request.deny)
            case .yes, .no:
                try await computer.connection.perform(
                    .answer(chat, request: request.id, answer: .confirm(yes: { if case .yes = reply { true } else { false } }())),
                    timeout: .act)
            case .text(let words):
                try await computer.connection.perform(
                    .answer(chat, request: request.id, answer: .input(text: words)), timeout: .act)
            }
            await Self.clear(thread: thread)
        } catch let failure as RemoteFailure where failure.code == "request_not_pending" {
            // Answered already, on the computer or here.
            await Self.clear(thread: thread)
        } catch {
            let content = UNMutableNotificationContent()
            content.title = opened.payload.title ?? computer.hostName
            content.body = "Could not reach \(computer.hostName). Open Ghost to answer."
            content.threadIdentifier = thread
            content.userInfo = [:]
            try? await UNUserNotificationCenter.current().add(
                UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
        }
    }

    /// Takes a chat's notifications away once what they asked is answered.
    static func clear(thread: String) async {
        let center = UNUserNotificationCenter.current()
        let delivered = await center.deliveredNotifications()
        center.removeDeliveredNotifications(
            withIdentifiers: delivered.filter { $0.request.content.threadIdentifier == thread }.map(\.request.identifier))
    }

    /// Writes what the widgets, the Live Activity and the badge show, a moment after the lists
    /// change.
    func scheduleSnapshot() {
        snapshotWrite?.cancel()
        snapshotWrite = Task { [weak self] in
            try? await Task.sleep(for: .seconds(1))
            guard !Task.isCancelled, let self else { return }
            self.writeSnapshot()
        }
    }

    func writeSnapshot() {
        let snapshot = SharedSnapshot(computers: computers.map { computer in
            let waiting = computer.needsYou.map { chat in
                SharedSnapshot.Chat(
                    id: chat.chat.id, title: chat.chat.title ?? "Untitled chat", agent: chat.chat.agentId,
                    state: "asking", updatedAt: chat.chat.updatedAt)
            }
            let working = computer.working.map { chat in
                SharedSnapshot.Chat(
                    id: chat.chat.id, title: chat.chat.title ?? "Untitled chat", agent: chat.chat.agentId,
                    state: "working", updatedAt: chat.chat.updatedAt)
            }
            return SharedSnapshot.Computer(
                hostId: computer.hostId, hostName: computer.hostName, asking: computer.needsYou.count,
                working: computer.working.count, chats: Array((waiting + working).prefix(3)),
                theme: computer.look.theme, dark: computer.look.appearance != "light")
        })
        // The Live Activity follows every time: leaving the app is when one starts.
        activities.follow(self)
        guard snapshot.computers != lastSnapshot?.computers else { return }
        lastSnapshot = snapshot
        snapshot.save()
        WidgetCenter.shared.reloadAllTimelines()
        UNUserNotificationCenter.current().setBadgeCount(snapshot.asking)
    }
}

/// One Live Activity per computer while its agents work, following the counts. The app keeps it
/// current while it runs; afterwards the computer does, through Apple, sealed.
@MainActor
final class LiveActivities {
    private var watching: Set<String> = []

    func follow(_ model: AppModel) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled, model.push.liveActivities else {
            activityLog.notice("Live Activities are off")
            endAll()
            return
        }
        for computer in model.computers where computer.offersActivity {
            guard let device = computer.record?.deviceId else { continue }
            let state = GhostActivityAttributes.ContentState(
                asking: computer.needsYou.count, working: computer.working.count, lead: lead(computer),
                since: computer.working.first.flatMap { computer.workingLine($0.chat.id)?.since }.map {
                    Date(timeIntervalSince1970: Double($0) / 1000)
                })
            let busy = state.asking + state.working > 0
            if Activity<GhostActivityAttributes>.activities.contains(where: {
                $0.attributes.device == device && ($0.activityState == .active || $0.activityState == .stale)
            }) {
                Task.detached { await Self.apply(state, device: device, busy: busy) }
            } else if busy {
                do {
                    let activity = try Activity.request(
                        attributes: GhostActivityAttributes(device: device),
                        content: ActivityContent(state: state, staleDate: nil), pushType: .token)
                    watch(activity.id, connection: computer.connection, environment: model.push.environment)
                } catch {
                    activityLog.notice("no Live Activity: \(error.localizedDescription, privacy: .public)")
                }
            }
        }
    }

    /// Updates the computer's activity, or ends it once nothing works or waits.
    private nonisolated static func apply(
        _ state: GhostActivityAttributes.ContentState, device: String, busy: Bool
    ) async {
        guard
            let activity = Activity<GhostActivityAttributes>.activities.first(where: {
                $0.attributes.device == device && ($0.activityState == .active || $0.activityState == .stale)
            })
        else { return }
        if busy {
            await activity.update(ActivityContent(state: state, staleDate: nil))
        } else {
            await activity.end(ActivityContent(state: state, staleDate: nil), dismissalPolicy: .default)
        }
    }

    /// Hands each new token for the activity's updates to the computer.
    private func watch(_ id: String, connection: HostConnection, environment: String) {
        guard !watching.contains(id) else { return }
        watching.insert(id)
        Task.detached {
            guard let activity = Activity<GhostActivityAttributes>.activities.first(where: { $0.id == id }) else { return }
            for await token in activity.pushTokenUpdates {
                let hex = token.map { String(format: "%02x", $0) }.joined()
                try? await connection.perform(.activity(token: hex, env: environment))
            }
        }
    }

    func endAll() {
        Task.detached {
            for activity in Activity<GhostActivityAttributes>.activities {
                await activity.end(nil, dismissalPolicy: .immediate)
            }
        }
    }

    /// The chat that leads, in plain words: the Lock Screen draws no Markdown.
    private func lead(_ computer: ComputerStore) -> String? {
        let words: String?
        if let chat = computer.needsYou.first {
            let ask = computer.asking(chat.chat.id)?.request?.title
            words = [chat.chat.title, ask].compactMap { $0 }.joined(separator: ": ")
        } else if let chat = computer.working.first {
            let line = computer.workingLine(chat.chat.id)?.line
            words = [chat.chat.agentName, line ?? chat.chat.title].compactMap { $0 }.joined(separator: ": ")
        } else {
            words = nil
        }
        return words?.replacingOccurrences(of: "`", with: "")
    }
}
