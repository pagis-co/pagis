import UserNotifications

/// The delegate of `UNUserNotificationCenter`. The app owns it, and
/// Capacitor does not (`ios.handleApplicationNotifications` is false in
/// `capacitor.config.json`), because the tap and the inline answer of a
/// Notification need it (ADR-0032).
final class NotificationResponder: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotificationResponder()

    /// The daemon holds a Notification while the Person is active in a
    /// client. So a Notification that arrives while the app is in front
    /// shows, as it does in a browser.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .list, .sound])
    }
}
