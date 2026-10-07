import UIKit
import Capacitor
import UserNotifications

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    /// The type of the **Change server** item in the menu of the app icon
    /// (`UIApplicationShortcutItems` in `Info.plist`).
    static let changeServer = "co.pagis.mobile.change-server"

    var window: UIWindow?

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options connectionOptions: UIScene.ConnectionOptions) {
        guard let windowScene = scene as? UIWindowScene else { return }

        if connectionOptions.shortcutItem?.type == SceneDelegate.changeServer {
            ServerStore().server = nil
            KeychainSessionCopy().delete()
        }
        // A tap on a Notification that starts the app: the bridge opens its
        // place first.
        if let response = connectionOptions.notificationResponse,
           response.actionIdentifier == UNNotificationDefaultActionIdentifier {
            NotificationTap.shared.open(response.notification)
        }
        let window = UIWindow(windowScene: windowScene)
        self.window = window
        PagisViewController.launch(in: window)

        SceneDelegateProxy.shared.scene(scene, willConnectTo: session, options: connectionOptions)
    }

    func windowScene(
        _ windowScene: UIWindowScene,
        performActionFor shortcutItem: UIApplicationShortcutItem,
        completionHandler: @escaping (Bool) -> Void
    ) {
        guard shortcutItem.type == SceneDelegate.changeServer,
              let bridge = window?.rootViewController as? PagisViewController
        else {
            completionHandler(false)
            return
        }
        bridge.changeServer()
        completionHandler(true)
    }

    /// The daemon sends no push when an item leaves the Needs-You Queue, so
    /// the app removes the stale Notifications when it comes to the
    /// foreground, and sets the badge to the count of the queue.
    func sceneWillEnterForeground(_ scene: UIScene) {
        Task { @MainActor in
            await StaleNotifications.app.clean()
        }
    }

    func sceneDidBecomeActive(_ scene: UIScene) {
        do {
            try AudioSessionSetup.configure()
        } catch {
            NSLog("Pagis did not set the audio session: %@", String(describing: error))
        }
    }

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        SceneDelegateProxy.shared.scene(scene, openURLContexts: URLContexts)
    }

    func scene(_ scene: UIScene, continue userActivity: NSUserActivity) {
        SceneDelegateProxy.shared.scene(scene, continue: userActivity)
    }
}
