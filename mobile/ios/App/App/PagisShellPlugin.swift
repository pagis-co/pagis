import Capacitor
import Foundation
import UserNotifications

/// The bridge of the app, as the plugin `PagisShell` sees it.
/// `PagisViewController` is one.
@MainActor
protocol ServerShell: AnyObject {
    /// Keep `server`, and start the bridge again at `firstPage`.
    func open(server: WebOrigin, firstPage: URL)
    /// Forget the server and the copy of the Session, and start the bridge
    /// again on the Connect screen.
    func changeServer()
    /// The Session ended, and the app opens the Connect screen.
    func sessionEnded()
}

/// `PagisShell`, the plugin of the app target. The Connect screen calls it
/// (`mobile/src/shell.ts`), and so does the Product App
/// (`ui/src/mobileShell.ts`).
@objc(PagisShellPlugin)
final class PagisShellPlugin: CAPPlugin, CAPBridgedPlugin {
    let identifier = "PagisShellPlugin"
    let jsName = "PagisShell"
    let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "buildType", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "open", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "changeServer", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "getLockScreenAnswers", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "setLockScreenAnswers", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "sessionEnded", returnType: CAPPluginReturnPromise)
    ]

    /// The bridge that shows the plugin.
    weak var shell: ServerShell?
    /// The store of the setting **Answer on the lock screen**.
    var servers = ServerStore()
    /// The categories that the app registers again when the setting
    /// changes.
    var categories: NotificationCategories = UNUserNotificationCenter.current()

    @objc func buildType(_ call: CAPPluginCall) {
        call.resolve(["debug": AppBuild.isDebug])
    }

    /// **Change server** of the You screen. The app forgets the server and
    /// the copy of the Session, and shows the Connect screen.
    @objc func changeServer(_ call: CAPPluginCall) {
        call.resolve()
        DispatchQueue.main.async { [weak self] in
            self?.shell?.changeServer()
        }
    }

    /// The setting **Answer on the lock screen** of this phone.
    @objc func getLockScreenAnswers(_ call: CAPPluginCall) {
        call.resolve(["on": servers.lockScreenAnswers])
    }

    /// Keep the setting **Answer on the lock screen**, and register the
    /// category `approval` again with or without its actions (ADR-0032).
    @objc func setLockScreenAnswers(_ call: CAPPluginCall) {
        guard let on = call.getBool("on") else {
            call.reject("The setting needs an on or off value.")
            return
        }
        servers.lockScreenAnswers = on
        DispatchQueue.main.async { [self] in
            InlineAnswer.registerCategory(servers: servers, in: categories)
            call.resolve()
        }
    }

    /// Keep the origin, and start the bridge again at the server with the
    /// first page `opens`. The shell checks both again: the page asks, and
    /// the native side decides.
    @objc func open(_ call: CAPPluginCall) {
        guard let server = WebOrigin.server(call.getString("origin") ?? "", debug: AppBuild.isDebug) else {
            call.reject("This is not the origin of a Pagis server that the app opens.")
            return
        }
        guard let firstPage = server.page(call.getString("opens") ?? "") else {
            call.reject("The first page is not on the origin of the server.")
            return
        }
        call.resolve()
        DispatchQueue.main.async { [weak self] in
            self?.shell?.open(server: server, firstPage: firstPage)
        }
    }

    /// Send the Product App the event `navigate` with the place of a tap on
    /// a Notification, and the Product App moves its router. The page does
    /// not load again. An event that comes before the Product App listens
    /// waits for its first listener.
    func navigate(to place: String) {
        notifyListeners("navigate", data: ["path": place], retainUntilConsumed: true)
    }

    /// The Product App says that the Session ended. The shell deletes the
    /// copy of the Session and opens the Connect screen.
    @objc func sessionEnded(_ call: CAPPluginCall) {
        call.resolve()
        DispatchQueue.main.async { [weak self] in
            self?.shell?.sessionEnded()
        }
    }
}
