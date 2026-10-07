import Foundation

/// What came of a read of the Needs-You Queue.
enum NeedsYouAnswer: Equatable {
    /// The ids of the items of the queue, and their count.
    case queue(items: Set<String>, count: Int)
    /// The daemon answered with another status, such as `401` for a
    /// Session that ended.
    case refused(status: Int)
    /// No answer came, or the answer is not a queue.
    case unreadable(String)
}

/// Reads the Needs-You Queue of the Person, `GET /api/v1/needs-you`, with
/// the copy of the Session (ADR-0030). The request has no `Origin` and no
/// `Sec-Fetch-Site`, so the cross-origin check of the daemon passes it to
/// the Session check as a request from a program (ADR-0024, ADR-0032).
struct NeedsYouRead {
    /// How long a read waits for the daemon, from start to end.
    static let timeout: TimeInterval = 10

    private let session: URLSession

    init(configuration: URLSessionConfiguration = .ephemeral, timeout: TimeInterval = NeedsYouRead.timeout) {
        let configuration = configuration.copy() as! URLSessionConfiguration
        configuration.timeoutIntervalForRequest = timeout
        configuration.timeoutIntervalForResource = timeout
        // The request sends the copy alone, and keeps no cookie that the
        // daemon sets.
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCache = nil
        session = URLSession(configuration: configuration)
    }

    func read(origin: WebOrigin, session copy: Session) async -> NeedsYouAnswer {
        guard let url = URL(string: "\(origin.serverURL)/api/v1/needs-you") else {
            preconditionFailure("The server origin \(origin.serverURL) makes no URL.")
        }
        var request = URLRequest(url: url)
        request.httpShouldHandleCookies = false
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("\(Session.cookieName)=\(copy.value)", forHTTPHeaderField: "Cookie")
        let data: Data
        let status: Int
        do {
            let (body, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                return .unreadable("the answer is not HTTP")
            }
            data = body
            status = http.statusCode
        } catch {
            return .unreadable(error.localizedDescription)
        }
        guard (200..<300).contains(status) else { return .refused(status: status) }
        return NeedsYouRead.queue(in: data)
    }

    /// The queue in the body `{items: [{id, ...}], count}`.
    private static func queue(in body: Data) -> NeedsYouAnswer {
        guard let object = try? JSONSerialization.jsonObject(with: body) as? [String: Any],
              let items = object["items"] as? [[String: Any]],
              let count = object["count"] as? Int
        else { return .unreadable("the answer is not a queue") }
        var ids = Set<String>()
        for item in items {
            guard let id = item["id"] as? String else { return .unreadable("an item of the queue has no id") }
            ids.insert(id)
        }
        return .queue(items: ids, count: count)
    }
}
