import AppKit
import RustCodeKit
import UserNotifications

/// Tells the user when a chat finishes or waits for them while RustCode is in
/// the background, and keeps the Dock badge at the number of waiting requests.
@MainActor
final class TurnNotifier: NSObject, UNUserNotificationCenterDelegate {
    static let shared = TurnNotifier()
    static let enabledKey = "notifyTurns"

    /// Opens the chat a clicked notification is about.
    var openSession: ((String) -> Void)?

    /// Notifications need a bundled app; `swift run` has none.
    private var center: UNUserNotificationCenter? {
        Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
    }

    var isEnabled: Bool {
        UserDefaults.standard.object(forKey: Self.enabledKey) as? Bool ?? true
    }

    func start() {
        guard let center else { return }
        center.delegate = self
        guard isEnabled else { return }
        center.requestAuthorization(options: [.alert, .sound, .badge]) { _, _ in }
    }

    /// Post only while the app is in the background: in front, the chat shows it.
    func notify(session: String, title: String, body: String) {
        guard isEnabled, !NSApp.isActive, let center else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.sound = .default
        content.userInfo = ["sessionId": session]
        content.threadIdentifier = session
        center.add(UNNotificationRequest(identifier: UUID().uuidString, content: content, trigger: nil))
    }

    func setBadge(_ count: Int) {
        NSApp.dockTile.badgeLabel = count > 0 ? String(count) : nil
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let session = response.notification.request.content.userInfo["sessionId"] as? String
        Task { @MainActor in
            NSApp.activate(ignoringOtherApps: true)
            if let session { self.openSession?(session) }
            completionHandler()
        }
    }

    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([])
    }
}
