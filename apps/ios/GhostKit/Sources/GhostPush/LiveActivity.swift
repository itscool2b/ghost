#if canImport(ActivityKit) && os(iOS)
public import ActivityKit
public import Foundation
import GhostCrypto

/// The Live Activity for one computer: how many chats need the person and how many work, and the
/// one that leads. Its updates come from the computer through Apple, sealed as notifications are:
/// the state Apple carries is `{"d": <device id>, "b": <sealed>}`, opened here, in the widget
/// extension, with the push key. Only the app's own updates are ever plain, on the phone.
public struct GhostActivityAttributes: ActivityAttributes {
    public struct ContentState: Codable, Hashable, Sendable {
        public var asking: Int
        public var working: Int
        /// What leads: the ask, or "Codex: Running cargo test".
        public var lead: String?
        /// When the leading work started, for the timer.
        public var since: Date?
        /// False when an update could not be opened (the phone not unlocked since it started):
        /// the activity shows Ghost's name only.
        public var opened: Bool

        public init(asking: Int, working: Int, lead: String? = nil, since: Date? = nil, opened: Bool = true) {
            self.asking = asking
            self.working = working
            self.lead = lead
            self.since = since
            self.opened = opened
        }

        private struct Plain: Codable {
            var asking: Int
            var working: Int
            var lead: String?
            /// Milliseconds.
            var since: Int64?
        }

        enum CodingKeys: String, CodingKey {
            case d, b, asking, working, lead, since
        }

        public init(from decoder: any Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            if let blob = try container.decodeIfPresent(String.self, forKey: .b) {
                let device = try container.decode(String.self, forKey: .d)
                guard let plain = ActivitySeal.open(blob, device: device),
                    let state = try? JSONDecoder().decode(Plain.self, from: plain)
                else {
                    self.init(asking: 0, working: 0, opened: false)
                    return
                }
                self.init(
                    asking: state.asking, working: state.working, lead: state.lead,
                    since: state.since.map { Date(timeIntervalSince1970: Double($0) / 1000) })
                return
            }
            self.init(
                asking: try container.decodeIfPresent(Int.self, forKey: .asking) ?? 0,
                working: try container.decodeIfPresent(Int.self, forKey: .working) ?? 0,
                lead: try container.decodeIfPresent(String.self, forKey: .lead),
                since: try container.decodeIfPresent(Int64.self, forKey: .since).map {
                    Date(timeIntervalSince1970: Double($0) / 1000)
                })
        }

        public func encode(to encoder: any Encoder) throws {
            var container = encoder.container(keyedBy: CodingKeys.self)
            try container.encode(asking, forKey: .asking)
            try container.encode(working, forKey: .working)
            try container.encodeIfPresent(lead, forKey: .lead)
            try container.encodeIfPresent(since.map { Int64($0.timeIntervalSince1970 * 1000) }, forKey: .since)
        }
    }

    /// The device the computer knows this phone by: whose key opens the updates, and whose
    /// computer the activity is for.
    public var device: String

    public init(device: String) {
        self.device = device
    }
}

/// Opens a Live Activity's sealed state with the device's push key and the computer it came from.
enum ActivitySeal {
    static func open(_ blob: String, device: String) -> Data? {
        let opener = PushOpener()
        guard let record = opener.records().first(where: { $0.deviceId == device }),
            let key = opener.keys.load(deviceId: device)
        else { return nil }
        return PushSeal.open(blob, key: key, hostId: record.hostId, deviceId: device)
    }
}
#endif
