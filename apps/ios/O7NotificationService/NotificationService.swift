import O7Push
import UserNotifications
import os

private let log = Logger(subsystem: "codes.o7.mobile", category: "notifications")

/// Opens each notification o7's computer sealed for this phone and writes its words in place of
/// Apple's fixed ones: the chat as the title, the project and agent under it, the ask or the
/// result as the body, and Allow or Deny only when the words opened. It needs no network.
final class NotificationService: UNNotificationServiceExtension {
    private var deliver: ((UNNotificationContent) -> Void)?
    private var fixed: UNNotificationContent?

    override func didReceive(
        _ request: UNNotificationRequest, withContentHandler deliver: @escaping (UNNotificationContent) -> Void
    ) {
        self.deliver = deliver
        fixed = request.content
        let opener = PushOpener()
        guard let content = request.content.mutableCopy() as? UNMutableNotificationContent,
            let opened = opener.open(request.content.userInfo)
        else {
            log.notice("not opened: \(opener.why(request.content.userInfo), privacy: .public)")
            // Not opened (the phone was not unlocked since it started, or a computer forgotten
            // since): Apple's fixed words stay, with no buttons.
            deliver(request.content)
            return
        }
        var snapshot = SharedSnapshot.load()
        if let asking = opened.payload.askingCount {
            snapshot.setAsking(asking, host: opened.hostId, name: opened.hostName)
            snapshot.save()
        }
        NotificationWords(opened).apply(to: content, badge: snapshot.asking)
        deliver(content)
    }

    override func serviceExtensionTimeWillExpire() {
        if let deliver, let fixed { deliver(fixed) }
    }
}
