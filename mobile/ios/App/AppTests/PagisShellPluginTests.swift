import Capacitor
import UserNotifications
import XCTest
@testable import App

/// The plugin `PagisShell` keeps the setting **Answer on the lock screen**
/// of this phone, and starts **Change server** in the bridge (ADR-0032).
@MainActor
final class PagisShellPluginTests: XCTestCase {
    private let defaults = UserDefaults(suiteName: "PagisShellPluginTests")!

    private var plugin: PagisShellPlugin!
    private var categories: FakeCategories!
    private var shell: FakeServerShell!

    override func setUp() async throws {
        defaults.removePersistentDomain(forName: "PagisShellPluginTests")
        categories = FakeCategories()
        shell = FakeServerShell()
        plugin = PagisShellPlugin()
        plugin.servers = ServerStore(defaults: defaults)
        plugin.categories = categories
        plugin.shell = shell
    }

    override func tearDown() async throws {
        defaults.removePersistentDomain(forName: "PagisShellPluginTests")
    }

    // MARK: - Answer on the lock screen

    func testTheSettingIsOffWhenThePhoneStoresNone() {
        XCTAssertFalse(ServerStore(defaults: defaults).lockScreenAnswers)
    }

    func testTheStoreKeepsTheSetting() {
        let servers = ServerStore(defaults: defaults)

        servers.lockScreenAnswers = true
        XCTAssertTrue(ServerStore(defaults: defaults).lockScreenAnswers)

        servers.lockScreenAnswers = false
        XCTAssertFalse(ServerStore(defaults: defaults).lockScreenAnswers)
    }

    func testGetLockScreenAnswersGivesTheStoredSetting() async throws {
        let off = try await resolve(plugin.getLockScreenAnswers)
        XCTAssertEqual(off?["on"] as? Bool, false)

        ServerStore(defaults: defaults).lockScreenAnswers = true

        let on = try await resolve(plugin.getLockScreenAnswers)
        XCTAssertEqual(on?["on"] as? Bool, true)
    }

    /// The app registers the category again, so a change of the setting
    /// does not wait for the next launch.
    func testSetLockScreenAnswersStoresTheSettingAndRegistersTheCategoryAgain() async throws {
        _ = try await resolve(plugin.setLockScreenAnswers, options: ["on": true])

        XCTAssertTrue(ServerStore(defaults: defaults).lockScreenAnswers)
        XCTAssertEqual(categories.registered.count, 1)
        XCTAssertEqual(categories.registered.last?.first?.actions.map(\.identifier), ["approve_once", "deny"])

        _ = try await resolve(plugin.setLockScreenAnswers, options: ["on": false])

        XCTAssertFalse(ServerStore(defaults: defaults).lockScreenAnswers)
        XCTAssertEqual(categories.registered.count, 2)
        XCTAssertEqual(categories.registered.last?.first?.actions.count, 0)
    }

    func testSetLockScreenAnswersWithNoValueChangesNothing() async {
        ServerStore(defaults: defaults).lockScreenAnswers = true

        let message = await reject(plugin.setLockScreenAnswers, options: [:])

        XCTAssertEqual(message, "The setting needs an on or off value.")
        XCTAssertTrue(ServerStore(defaults: defaults).lockScreenAnswers)
        XCTAssertTrue(categories.registered.isEmpty)
    }

    // MARK: - Change server

    func testChangeServerResolvesAndChangesTheServerOfTheBridge() async throws {
        let changed = expectation(description: "the bridge changes the server")
        shell.onChangeServer = { changed.fulfill() }

        _ = try await resolve(plugin.changeServer)
        await fulfillment(of: [changed], timeout: 5)

        XCTAssertEqual(shell.changes, 1)
    }

    // MARK: - Helpers

    private struct Rejected: Error {
        let message: String
    }

    /// Call `method` with `options`, and give the data that it resolves.
    private func resolve(
        _ method: (CAPPluginCall) -> Void,
        options: [String: Any] = [:]
    ) async throws -> [String: Any]? {
        try await withCheckedThrowingContinuation { continuation in
            method(CAPPluginCall(
                callbackId: "test",
                methodName: "test",
                options: options,
                success: { result, _ in continuation.resume(returning: result?.data) },
                error: { error in continuation.resume(throwing: Rejected(message: error?.message ?? "")) }
            ))
        }
    }

    /// Call `method` with `options`, and give the message of its rejection,
    /// or nil when it resolves.
    private func reject(_ method: (CAPPluginCall) -> Void, options: [String: Any]) async -> String? {
        do {
            _ = try await resolve(method, options: options)
            return nil
        } catch let rejected as Rejected {
            return rejected.message
        } catch {
            return nil
        }
    }
}

/// The bridge of the app. It counts each change of the server.
@MainActor
final class FakeServerShell: ServerShell {
    var changes = 0
    var onChangeServer: () -> Void = {}

    func open(server: WebOrigin, firstPage: URL) {}

    func changeServer() {
        changes += 1
        onChangeServer()
    }

    func sessionEnded() {}
}
