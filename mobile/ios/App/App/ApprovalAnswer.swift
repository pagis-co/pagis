import Foundation

/// The decision that an action of a Notification of an Approval posts. A
/// Notification approves once only: an Allow Rule needs the approval
/// card, which states what the rule covers (ADR-0032).
enum ApprovalDecision: String {
    case approved
    case denied

    /// The decision of the action `approve_once` or `deny` of the
    /// `approval` category, or nil for each other action, such as the tap
    /// on the body.
    init?(action: String) {
        switch action {
        case "approve_once": self = .approved
        case "deny": self = .denied
        default: return nil
        }
    }
}

/// What came of an answer.
enum ApprovalAnswerOutcome: Equatable {
    /// The daemon recorded the decision.
    case taken
    /// The daemon answered with another status, such as `401` for a
    /// Session that ended or `409` for a Request that is decided already.
    case refused(status: Int)
    /// No answer came: no network, or no answer in the time.
    case unreachable(String)
}

/// Posts a decision to the decision route of the daemon,
/// `POST /api/v1/requests/{request_id}/decision`, with the copy of the
/// Session. The request has no `scope`, so the decision is `once` and
/// writes no Allow Rule. It has no `Origin` and no `Sec-Fetch-Site`, so
/// the cross-origin check of the daemon passes it to the Session check as
/// a request from a program (ADR-0024, ADR-0032).
struct ApprovalAnswer {
    /// How long an answer waits for the daemon, from start to end. The
    /// service worker waits as long.
    static let timeout: TimeInterval = 20

    private let session: URLSession

    init(configuration: URLSessionConfiguration = .ephemeral, timeout: TimeInterval = ApprovalAnswer.timeout) {
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

    func post(
        _ decision: ApprovalDecision,
        request id: String,
        origin: WebOrigin,
        session copy: Session
    ) async -> ApprovalAnswerOutcome {
        var request = URLRequest(url: ApprovalAnswer.route(of: id, at: origin))
        request.httpMethod = "POST"
        request.httpShouldHandleCookies = false
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue("\(Session.cookieName)=\(copy.value)", forHTTPHeaderField: "Cookie")
        request.httpBody = try? JSONSerialization.data(withJSONObject: ["decision": decision.rawValue])
        do {
            let (_, response) = try await session.data(for: request)
            guard let status = (response as? HTTPURLResponse)?.statusCode else {
                return .unreachable("the answer is not HTTP")
            }
            return (200..<300).contains(status) ? .taken : .refused(status: status)
        } catch {
            return .unreachable(error.localizedDescription)
        }
    }

    /// The decision route of the Request `id`, with the id as one path
    /// segment.
    private static func route(of id: String, at origin: WebOrigin) -> URL {
        let segment = id.addingPercentEncoding(withAllowedCharacters: unreserved) ?? ""
        guard let url = URL(string: "\(origin.serverURL)/api/v1/requests/\(segment)/decision") else {
            preconditionFailure("The server origin \(origin.serverURL) makes no URL.")
        }
        return url
    }

    /// The unreserved characters of RFC 3986, which a path segment holds
    /// as they are.
    private static let unreserved = CharacterSet(
        charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~"
    )
}
