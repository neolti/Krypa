import QuartzCore
import UIKit

/// A full-screen view backed by a CAMetalLayer. Rust draws into the layer; this class only
/// reports size, scale, safe area and touches, and drives the display link.
final class KrypaMetalView: UIView {
    override class var layerClass: AnyClass { CAMetalLayer.self }

    private var handle: OpaquePointer?  // KrypaView*
    private var displayLink: CADisplayLink?
    private var dead = false

    private var metalLayer: CAMetalLayer { layer as! CAMetalLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isMultipleTouchEnabled = true
        isOpaque = true
        backgroundColor = .black
    }

    required init?(coder: NSCoder) { fatalError("not used") }

    deinit {
        displayLink?.invalidate()
        krypa_view_destroy(handle)
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
        guard window != nil, !dead, bounds.width > 0, bounds.height > 0 else { return }
        metalLayer.contentsScale = scale
        let (w, h) = pixelSize
        if handle == nil {
            var out: OpaquePointer?
            let status = krypa_view_create(
                Unmanaged.passUnretained(metalLayer).toOpaque(), w, h, Float(scale), rustInsets, &out)
            // Fail loud: a launcher that silently does not draw is worse than a crash while
            // this is a development shell.
            guard status == Int32(KRYPA_OK), out != nil else {
                fatalError("krypa_view_create failed with status \(status)")
            }
            handle = out
            NSLog("krypa: view created \(w)x\(h) px, scale \(scale)")
        } else {
            check(krypa_view_resize(handle, w, h, Float(scale), rustInsets), "resize")
        }
        wake()
    }

    // MARK: drawing (only when Rust says something changes)

    private func wake() {
        displayLink?.isPaused = false
    }

    @objc private func tick() {
        guard let handle, !dead else { return }
        var nextMs: UInt32 = 0
        let status = krypa_view_render(handle, CACurrentMediaTime(), &nextMs)
        guard check(status, "render") else { return }
        if nextMs == 0 { return }  // keep the link running: an animation is active
        displayLink?.isPaused = true
        if nextMs != UInt32(KRYPA_IDLE) {
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(Int(nextMs))) { [weak self] in
                self?.wake()
            }
        }
    }

    @discardableResult
    private func check(_ status: Int32, _ what: String) -> Bool {
        if status == Int32(KRYPA_OK) { return true }
        NSLog("krypa: \(what) failed with status \(status)")
        if status == Int32(KRYPA_ERR_PANIC) {
            dead = true  // the Rust handle is poisoned; stop calling it
            displayLink?.isPaused = true
        }
        return false
    }

    // MARK: touches (points, as UIKit reports them)

    override func touchesBegan(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_BEGAN)) }
    override func touchesMoved(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_MOVED)) }
    override func touchesEnded(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_ENDED)) }
    override func touchesCancelled(_ touches: Set<UITouch>, with event: UIEvent?) { forward(touches, UInt32(KRYPA_TOUCH_CANCELLED)) }

    private func forward(_ touches: Set<UITouch>, _ phase: UInt32) {
        guard let handle, !dead else { return }
        for touch in touches {
            let id = UInt64(UInt(bitPattern: Unmanaged.passUnretained(touch).toOpaque()))
            let p = touch.location(in: self)
            check(krypa_view_touch(handle, id, phase, Float(p.x), Float(p.y)), "touch")
        }
        wake()
    }
}
