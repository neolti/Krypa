import Security
import UIKit
import WebKit

/// Executes the commands Rust issues (one WKWebView per session) and reports what the web
/// views do. The core owns every decision; this class never decides policy, never computes
/// a security verdict and never opens a link by itself. Rust rejects whatever is stale.
final class KrypaEngine {
    private weak var host: KrypaMetalView?
    private let container: UIView
    private var sessions: [UInt64: WebSession] = [:]
    private var draining = false
    private var lastPlacementLog = ""

    init(container: UIView, host: KrypaMetalView) {
        self.container = container
        self.host = host
    }

    var core: KrypaCore? { host?.core }

    // MARK: Rust -> web views

    /// After every frame: run the queued commands, then put the web view where Rust says.
    func afterFrame() {
        drain()
        applyPlacement()
    }

    func drain() {
        guard !draining, let core else { return }
        draining = true
        defer { draining = false }
        var ran = false
        while let command = core.nextCommand() {
            execute(command)
            ran = true
        }
        if ran { host?.wake() }
    }

    private func execute(_ command: [String: Any]) {
        guard let (name, body) = command.first, let args = body as? [String: Any],
            let session = (args["session"] as? NSNumber)?.uint64Value
        else {
            NSLog("krypa: unreadable command")
            return
        }
        let url = (args["url"] as? String).flatMap { URL(string: $0) }
        switch name {
        case "CreateSession":
            guard args["profile"] as? String == "Default" else {
                // The core only sends other profiles if a capability says they exist.
                NSLog("krypa: unsupported profile kind")
                return
            }
            let created = WebSession(id: session, engine: self)
            sessions[session] = created
            if let initial = (args["initial_url"] as? String).flatMap({ URL(string: $0) }) {
                created.load(initial)
            }
        case "DestroySession":
            if let s = sessions.removeValue(forKey: session) { s.teardown() }
            send("SessionDestroyed", ["session": session])
        case "Load":
            if let url { sessions[session]?.load(url) }
        case "Reload": sessions[session]?.reload()
        case "Stop": sessions[session]?.webView.stopLoading()
        case "GoBack": sessions[session]?.webView.goBack()
        case "GoForward": sessions[session]?.webView.goForward()
        case "Show", "Hide":
            break  // placement-driven: Rust says which web view is on screen after each frame
        case "OpenExternal":
            // Only sent after the user confirmed in Krypa's own dialog.
            if let url { UIApplication.shared.open(url, options: [:], completionHandler: nil) }
        default:
            // Suspend/Resume (view_suspend is false), FetchManifest (manifest_fetch is
            // false), RespondToPermission (no prompt is ever raised): never sent.
            NSLog("krypa: command not implemented: \(name)")
        }
    }

    private func applyPlacement() {
        guard let p = core?.placement() else { return }
        let label =
            p.session == 0 ? "none" : "session=\(p.session) x=\(p.x) y=\(p.y) w=\(p.width) h=\(p.height)"
        if label != lastPlacementLog {
            lastPlacementLog = label
            ciLog("placement \(label)")
        }
        guard p.session != 0, let shown = sessions[p.session], p.width >= 1, p.height >= 1 else {
            for s in sessions.values where s.webView.superview != nil { s.webView.removeFromSuperview() }
            return
        }
        for (id, s) in sessions where id != p.session && s.webView.superview != nil {
            s.webView.removeFromSuperview()
        }
        if shown.webView.superview !== container { container.addSubview(shown.webView) }
        let frame = CGRect(x: CGFloat(p.x), y: CGFloat(p.y), width: CGFloat(p.width), height: CGFloat(p.height))
        if shown.webView.frame != frame { shown.webView.frame = frame }
    }

    // MARK: web views -> Rust

    func send(_ name: String, _ body: [String: Any]) {
        core?.push([name: body])
        host?.wake()
        drain()
    }

    func mayNavigate(session: UInt64, url: URL, target: Int, kind: Int, userGesture: Bool) -> Bool {
        let allowed =
            core?.mayNavigate(
                session: session, url: url, target: UInt32(target), kind: UInt32(kind), userGesture: userGesture)
            ?? false
        // The decision may have queued commands (a new session for a new window).
        host?.wake()
        return allowed
    }
}

/// One session's WKWebView and its delegates.
final class WebSession: NSObject, WKNavigationDelegate, WKUIDelegate {
    let id: UInt64
    let webView: WKWebView
    private weak var engine: KrypaEngine?
    private var observations: [NSKeyValueObservation] = []
    private var committed = false
    private var lastRequested: URL?
    private var lastLocationKey = ""

    init(id: UInt64, engine: KrypaEngine) {
        self.id = id
        self.engine = engine
        let config = WKWebViewConfiguration()
        // Profiles: ProfileKind::Default is the platform default data store (persistent).
        // Persistent and Private profiles are not offered yet (capabilities say so).
        config.websiteDataStore = WKWebsiteDataStore.default()
        config.defaultWebpagePreferences.allowsContentJavaScript = true
        // A script may open a window only from a user gesture; the core then decides.
        config.preferences.javaScriptCanOpenWindowsAutomatically = false
        config.preferences.isFraudulentWebsiteWarningEnabled = true
        config.allowsInlineMediaPlayback = true
        config.limitsNavigationsToAppBoundDomains = false
        // Deliberately none: no WKScriptMessageHandler, no user scripts (no JS-to-native
        // bridge for pages, App Review 4.7.2), no file access (file: is cancelled by policy).
        webView = WKWebView(frame: .zero, configuration: config)
        super.init()
        webView.navigationDelegate = self
        webView.uiDelegate = self
        #if KRYPA_DEBUG_START_URL
        if #available(iOS 16.4, *) { webView.isInspectable = true }
        #endif
        observe()
    }

    private func observe() {
        observations = [
            webView.observe(\.title, options: [.new]) { [weak self] _, _ in self?.titleChanged() },
            webView.observe(\.estimatedProgress, options: [.new]) { [weak self] _, _ in self?.progressChanged() },
            webView.observe(\.url, options: [.new]) { [weak self] _, _ in self?.locationChanged() },
            webView.observe(\.canGoBack, options: [.new]) { [weak self] _, _ in self?.locationChanged() },
            webView.observe(\.canGoForward, options: [.new]) { [weak self] _, _ in self?.locationChanged() },
            webView.observe(\.hasOnlySecureContent, options: [.new]) { [weak self] _, _ in self?.securityFactsChanged() },
        ]
    }

    func teardown() {
        observations.removeAll()
        webView.stopLoading()
        webView.navigationDelegate = nil
        webView.uiDelegate = nil
        webView.removeFromSuperview()
    }

    // MARK: commands

    func load(_ url: URL) {
        lastRequested = url
        webView.load(URLRequest(url: url))
    }

    func reload() {
        // After a failed first load or a killed content process there is nothing to reload:
        // load the last requested URL again instead.
        if webView.url == nil, let url = lastRequested {
            webView.load(URLRequest(url: url))
        } else {
            webView.reload()
        }
    }

    // MARK: events

    private func fields(_ url: URL) -> [String: Any] {
        [
            "session": id, "url": url.absoluteString,
            "can_go_back": webView.canGoBack, "can_go_forward": webView.canGoForward,
        ]
    }

    private var trustFact: String {
        guard webView.url?.scheme == "https", let trust = webView.serverTrust else { return "Absent" }
        var error: CFError?
        return SecTrustEvaluateWithError(trust, &error) ? "Valid" : "Invalid"
    }

    private func titleChanged() {
        guard committed, let title = webView.title else { return }
        engine?.send("Title", ["session": id, "title": String(title.prefix(512))])
    }

    private func progressChanged() {
        let permille = max(0, min(1000, Int((webView.estimatedProgress * 1000).rounded())))
        engine?.send("Progress", ["session": id, "permille": permille])
    }

    /// Same-document URL changes and history flag changes of the committed document.
    private func locationChanged() {
        guard committed, let url = webView.url else { return }
        let key = "\(url.absoluteString)|\(webView.canGoBack)|\(webView.canGoForward)"
        if key == lastLocationKey { return }
        lastLocationKey = key
        engine?.send("LocationChanged", fields(url))
    }

    private func securityFactsChanged() {
        guard committed else { return }
        engine?.send(
            "SecurityFacts",
            ["session": id, "trust": trustFact, "only_secure_content": webView.hasOnlySecureContent])
    }

    private func failed(_ error: Error) {
        committed = false
        let ns = error as NSError
        ciLog("load-failed domain=\(ns.domain) code=\(ns.code)")
        engine?.send("LoadFailed", ["session": id, "domain": String(ns.domain.prefix(64)), "code": ns.code])
    }

    // MARK: WKNavigationDelegate

    func webView(
        _ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction,
        decisionHandler: @escaping (WKNavigationActionPolicy) -> Void
    ) {
        guard let url = navigationAction.request.url, let engine else {
            decisionHandler(.cancel)
            return
        }
        let target: Int
        if navigationAction.targetFrame == nil {
            target = Int(KRYPA_NAV_TARGET_NEW_WINDOW)  // target=_blank and friends
        } else if navigationAction.targetFrame?.isMainFrame == true {
            target = Int(KRYPA_NAV_TARGET_MAIN)
            lastRequested = url
        } else {
            target = Int(KRYPA_NAV_TARGET_SUB)
        }
        // WKNavigationAction has no user-gesture flag. A tapped link or a submitted form is
        // the best signal there is; everything else is reported as "no gesture".
        let kind: Int
        var gesture = false
        switch navigationAction.navigationType {
        case .linkActivated:
            kind = Int(KRYPA_NAV_KIND_LINK)
            gesture = true
        case .formSubmitted:
            kind = Int(KRYPA_NAV_KIND_FORM)
            gesture = true
        case .reload: kind = Int(KRYPA_NAV_KIND_RELOAD)
        case .backForward: kind = Int(KRYPA_NAV_KIND_BACK_FORWARD)
        default: kind = Int(KRYPA_NAV_KIND_OTHER)
        }
        let allowed = engine.mayNavigate(session: id, url: url, target: target, kind: kind, userGesture: gesture)
        decisionHandler(allowed ? .allow : .cancel)
    }

    func webView(_ webView: WKWebView, didStartProvisionalNavigation navigation: WKNavigation!) {
        committed = false
        lastLocationKey = ""
        guard let url = lastRequested ?? webView.url else { return }
        engine?.send("NavigationStarted", fields(url))
    }

    func webView(_ webView: WKWebView, didCommit navigation: WKNavigation!) {
        guard let url = webView.url else { return }
        committed = true
        var event = fields(url)
        let trust = trustFact
        event["trust"] = trust
        event["only_secure_content"] = webView.hasOnlySecureContent
        ciLog("committed trust=\(trust) only-secure=\(webView.hasOnlySecureContent)")
        engine?.send("DocumentCommitted", event)
        lastLocationKey = "\(url.absoluteString)|\(webView.canGoBack)|\(webView.canGoForward)"
        titleChanged()
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        guard let url = webView.url else { return }
        engine?.send("NavigationFinished", fields(url))
        engine?.send("Progress", ["session": id, "permille": 1000])
        securityFactsChanged()
        ciLog("finished")
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        failed(error)
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        committed = false
        ciLog("content-process-terminated")
        engine?.send("ContentProcessTerminated", ["session": id])
    }

    /// Server trust and every other challenge: the system's default handling, never an
    /// override. A certificate the system rejects fails the load (reported as Tls).
    func webView(
        _ webView: WKWebView, didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        completionHandler(.performDefaultHandling, nil)
    }

    // MARK: WKUIDelegate

    /// window.open (only after a user gesture, see the configuration). The core decides
    /// whether it becomes a new session; this method never returns a web view, so the
    /// request is never silently dropped by Swift: Rust either opens a session or blocks it.
    func webView(
        _ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration,
        for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures
    ) -> WKWebView? {
        if let url = navigationAction.request.url {
            _ = engine?.mayNavigate(
                session: id, url: url, target: Int(KRYPA_NAV_TARGET_NEW_WINDOW),
                kind: Int(KRYPA_NAV_KIND_OTHER), userGesture: true)
        }
        return nil
    }

    /// Camera and microphone: denied by default; there is no permission UI in this milestone.
    func webView(
        _ webView: WKWebView, requestMediaCapturePermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo, type: WKMediaCaptureType,
        decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        decisionHandler(.deny)
    }

    /// Device motion and orientation: denied by default as well.
    func webView(
        _ webView: WKWebView, requestDeviceOrientationAndMotionPermissionFor origin: WKSecurityOrigin,
        initiatedByFrame frame: WKFrameInfo, decisionHandler: @escaping (WKPermissionDecision) -> Void
    ) {
        decisionHandler(.deny)
    }
}
