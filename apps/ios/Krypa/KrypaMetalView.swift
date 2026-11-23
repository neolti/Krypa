import QuartzCore
import UIKit

/// A full-screen view backed by a CAMetalLayer. Rust draws into the layer; this class only
/// reports size, scale, safe area and touches, and drives the display link. The web view is
/// a sibling above it (see KrypaEngine), never part of this layer.
final class KrypaMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }

    private(set) var core: KrypaCore?
    private var displayLink: CADisplayLink?
    private var wakeRequested = false

    /// Called on the main thread after every rendered frame (the engine drains commands and
    /// positions the web view there).
    var onFrame: (() -> Void)?

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isMultipleTouchEnabled = true
        isOpaque = true
        backgroundColor = .black
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    deinit {
        core = nil  // destroys the Rust view while the layer is still alive
        displayLink?.invalidate()
    }

    // MARK: geometry

    private var scale: CGFloat { window?.screen.scale ?? traitCollection.displayScale }

    private var pixelSize: (UInt32, UInt32) {
        (UInt32((bounds.width * scale).rounded()), UInt32((bounds.height * scale).rounded()))
    }

    private var rustInsets: KrypaInsets {
        let i = safeAreaInsets
        return KrypaInsets(top: Float(i.top), left: Float(i.left), bottom: Float(i.bottom), right: Float(i.right))
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        guard window != nil else {
            displayLink?.invalidate()
            displayLink = nil
            return
        }
        let link = CADisplayLink(target: self, selector: #selector(tick))
        link.add(to: .main, forMode: .common)
        displayLink = link
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        syncGeometry()
    }

    override func safeAreaInsetsDidChange() {
        super.safeAreaInsetsDidChange()
        syncGeometry()
    }

    /// Creates the Rust view on the first layout with a real size, then reports changes.
    private func syncGeometry() {
        guard window != nil, bounds.width > 0, bounds.height > 0 else { return }
        metalLayer.contentsScale = scale
        let (w, h) = pixelSize
        if let core {
            core.resize(widthPx: w, heightPx: h, scale: Float(scale), insets: rustInsets)
        } else {
            // Fail loud: a launcher that silently does not draw is worse than a crash while
            // this is a development shell.
            guard
                let created = KrypaCore(
                    layer: metalLayer, widthPx: w, heightPx: h, scale: Float(scale), insets: rustInsets)
            else { fatalError("krypa_view_create failed") }
            core = created
            NSLog("krypa: view created \(w)x\(h) px, scale \(scale)")
            #if KRYPA_DEBUG_START_URL
            // Test-only (Debug configuration): `-KrypaStartURL <url>` launch argument.
            if let start = UserDefaults.standard.string(forKey: "KrypaStartURL") {
                created.debugOpen(start)
            }
            #endif
        }
        wake()
    }

    // MARK: drawing (only when Rust says something changes)

    /// Keeps the display link running for at least one more frame.
    func wake() {
        wakeRequested = true
        displayLink?.isPaused = false
    }

    @objc private func tick() {
        guard let core, !core.dead else {
            displayLink?.isPaused = true
            return
        }
        wakeRequested = false
        guard let nextMs = core.render(time: CACurrentMediaTime()) else {
            if core.dead { displayLink?.isPaused = true }
            return
        }
        onFrame?()
        // Something woke us during the frame (commands executed, events pushed): draw again.
        if wakeRequested || nextMs == 0 { return }
        displayLink?.isPaused = true
        if nextMs != UInt32(KRYPA_IDLE) {
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Int(nextMs))) { [weak self] in
                self?.wake()
            }
        }
    }

    // MARK: touches (points, as UIKit reports them)

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_BEGAN)) }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_MOVED)) }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_ENDED)) }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_CANCELLED)) }

    private func forward(_ touches: Set<UITouch>, _ phase: UInt32) {
        guard let core, !core.dead else { return }
        for touch in touches {
            let id = UInt64(UInt(bitPattern: Unmanaged.passUnretained(touch).toOpaque()))
            let p = touch.location(in: self)
            core.touch(id: id, phase: phase, x: Float(p.x), y: Float(p.y))
        }
        wake()
    }
}
