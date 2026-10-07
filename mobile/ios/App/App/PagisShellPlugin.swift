import Capacitor
import Foundation

/// `PagisShell`, the plugin of the app target that the Connect screen
/// calls (`mobile/src/shell.ts`).
@objc(PagisShellPlugin)
final class PagisShellPlugin: CAPPlugin, CAPBridgedPlugin {
    let identifier = "PagisShellPlugin"
    let jsName = "PagisShell"
    let pluginMethods: [CAPPluginMethod] = [
        CAPPluginMethod(name: "buildType", returnType: CAPPluginReturnPromise),
        CAPPluginMethod(name: "open", returnType: CAPPluginReturnPromise)
    ]

    @objc func buildType(_ call: CAPPluginCall) {
        call.resolve(["debug": AppBuild.isDebug])
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
            (self?.bridge?.viewController as? PagisViewController)?.open(server: server, firstPage: firstPage)
        }
    }
}
