import UIKit

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = RootViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}

/// Hosts the Rust-drawn view full screen. The web view (WKWebView) will join it as a
/// sibling later.
final class RootViewController: UIViewController {
    override func loadView() {
        view = KrypaMetalView()
    }
}
