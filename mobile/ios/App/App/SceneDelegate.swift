import UIKit
import Capacitor

class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    /// The type of the **Change server** item in the menu of the app icon
    /// (`UIApplicationShortcutItems` in `Info.plist`).
    static let changeServer = "app.pagis.mobile.change-server"

    var window: UIWindow?

    func scene(_ scene: UIScene, willConnectTo session: UISceneSession, options connectionOptions: UIScene.ConnectionOptions) {
        guard let windowScene = scene as? UIWindowScene else { return }

        if connectionOptions.shortcutItem?.type == SceneDelegate.changeServer {
            ServerStore().server = nil
        }
        window = UIWindow(windowScene: windowScene)
        window?.rootViewController = PagisViewController()
        window?.makeKeyAndVisible()

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

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        SceneDelegateProxy.shared.scene(scene, openURLContexts: URLContexts)
    }

    func scene(_ scene: UIScene, continue userActivity: NSUserActivity) {
        SceneDelegateProxy.shared.scene(scene, continue: userActivity)
    }
}
