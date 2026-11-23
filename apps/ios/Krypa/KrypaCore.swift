import Foundation
import QuartzCore

/// Test-only log line the CI script waits for. Compiled out unless the debug start-URL
/// feature is on (Debug configuration only), so a release build prints nothing.
func ciLog(_ message: @autoclosure () -> String) {
    #if KRYPA_DEBUG_START_URL
    FileHandle.standardError.write(Data("krypa-ci: \(message())\n".utf8))
    #endif
}

/// Owns the Rust view handle and is the only place that calls the C ABI. Everything that
/// crosses (events, commands, navigation questions) is checked for a status code; Rust
/// treats what Swift sends as untrusted, and Swift treats a poisoned handle as dead.
final class KrypaCore {
    private var handle: OpaquePointer?  // KrypaView*
    private(set) var dead = false

    /// Creates the Rust view for `layer`. Returns nil (and logs) on failure.
    init?(layer: CAMetalLayer, widthPx: UInt32, heightPx: UInt32, scale: Float, insets: KrypaInsets) {
        var out: OpaquePointer?
        let status = krypa_view_create(
            Unmanaged.passUnretained(layer).toOpaque(), widthPx, heightPx, scale, insets, &out)
        guard status == Int32(KRYPA_OK), out != nil else {
            NSLog("krypa: view create failed with status \(status)")
            return nil
        }
        handle = out
    }

    deinit { krypa_view_destroy(handle) }

    @discardableResult
    private func check(_ status: Int32, _ what: String) -> Bool {
        if status == Int32(KRYPA_OK) { return true }
        // A refused event is normal (a late event of a closed session): quiet.
        if status != Int32(KRYPA_ERR_REJECTED) { NSLog("krypa: \(what) failed with status \(status)") }
        if status == Int32(KRYPA_ERR_PANIC) { dead = true }
        return false
    }

    // MARK: drawing and input

    func resize(widthPx: UInt32, heightPx: UInt32, scale: Float, insets: KrypaInsets) {
        guard let handle, !dead else { return }
        check(krypa_view_resize(handle, widthPx, heightPx, scale, insets), "resize")
    }

    func touch(id: UInt64, phase: UInt32, x: Float, y: Float) {
        guard let handle, !dead else { return }
        check(krypa_view_touch(handle, id, phase, x, y), "touch")
    }

    /// Returns the milliseconds until the next frame (0 = next tick, KRYPA_IDLE = never), or
    /// nil if rendering failed.
    func render(time: Double) -> UInt32? {
        guard let handle, !dead else { return nil }
        var next: UInt32 = 0
        guard check(krypa_view_render(handle, time, &next), "render") else { return nil }
        return next
    }

    // MARK: engine adapter

    /// Sends one fact about the web view to Rust as JSON.
    func push(_ event: [String: Any]) {
        guard let handle, !dead, JSONSerialization.isValidJSONObject(event),
            let data = try? JSONSerialization.data(withJSONObject: event)
        else { return }
        let status = data.withUnsafeBytes { (raw: UnsafeRawBufferPointer) -> Int32 in
            guard let base = raw.bindMemory(to: UInt8.self).baseAddress else {
                return Int32(KRYPA_ERR_INVALID_ARG)
            }
            return krypa_view_push_event(handle, base, data.count)
        }
        check(status, "push event")
    }

    /// The next command Rust wants executed, or nil when none is queued.
    func nextCommand() -> [String: Any]? {
        guard let handle, !dead else { return nil }
        var buffer = [UInt8](repeating: 0, count: 4096)
        var length = 0
        var status = buffer.withUnsafeMutableBufferPointer { b in
            krypa_view_next_command(handle, b.baseAddress, b.count, &length)
        }
        if status == Int32(KRYPA_ERR_BUFFER_TOO_SMALL) {
            buffer = [UInt8](repeating: 0, count: length)
            status = buffer.withUnsafeMutableBufferPointer { b in
                krypa_view_next_command(handle, b.baseAddress, b.count, &length)
            }
        }
        guard check(status, "next command"), length > 0, length <= buffer.count else { return nil }
        let json = try? JSONSerialization.jsonObject(with: Data(buffer[0..<length]))
        return json as? [String: Any]
    }

    /// Whether the web view may perform a navigation. Any error means no.
    func mayNavigate(session: UInt64, url: URL, target: UInt32, kind: UInt32, userGesture: Bool) -> Bool {
        guard let handle, !dead else { return false }
        let bytes = Array(url.absoluteString.utf8)
        var decision = UInt32(KRYPA_NAV_CANCEL)
        let status = bytes.withUnsafeBufferPointer { b in
            krypa_view_decide_navigation(
                handle, session, b.baseAddress, b.count, target, kind, userGesture ? 1 : 0, &decision)
        }
        return check(status, "decide navigation") && decision == UInt32(KRYPA_NAV_ALLOW)
    }

    /// Which web view to show and where (points), after the last render.
    func placement() -> KrypaPlacement? {
        guard let handle, !dead else { return nil }
        var p = KrypaPlacement()
        guard check(krypa_view_web_placement(handle, &p), "placement") else { return nil }
        return p
    }

    #if KRYPA_DEBUG_START_URL
    /// TEST ONLY: open a start URL without a keyboard.
    func debugOpen(_ url: String) {
        guard let handle, !dead else { return }
        let bytes = Array(url.utf8)
        let status = bytes.withUnsafeBufferPointer { b in
            krypa_debug_open_url(handle, b.baseAddress, b.count)
        }
        check(status, "debug open url")
        ciLog("debug-start-url status=\(status)")
    }
    #endif
}
