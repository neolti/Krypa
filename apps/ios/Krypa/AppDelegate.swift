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

/// Hosts the Rust-drawn view full screen with the web view as a sibling above it. The
/// engine puts the web view of the foreground session into the content rectangle Rust
/// reports; the Metal view stays underneath and draws the chrome around it.
final class RootViewController: UIViewController {
    private var engine: KrypaEngine?

    override func loadView() {
        let container = UIView()
        container.backgroundColor = .black
        view = container
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        let metal = KrypaMetalView(frame: view.bounds)
        metal.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(metal)
        let engine = KrypaEngine(container: view, host: metal)
        metal.onFrame = { [weak engine] in engine?.afterFrame() }
        self.engine = engine
    }
}
