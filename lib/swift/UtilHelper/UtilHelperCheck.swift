import Foundation
import AVFoundation

// Run with UtilHelper.swift; requires existing microphone authorization.
private final class CaptureCheck {
    let lock = NSLock()
    let began = ProcessInfo.processInfo.systemUptime
    var first: Double?
    var frames = 0
    var routeChanges = 0
    func receive(_ data: UnsafePointer<Float>?, _ count: UInt32) {
        lock.lock(); defer { lock.unlock() }
        if data == nil { routeChanges += 1; return }
        if first == nil { first = ProcessInfo.processInfo.systemUptime }
        frames += Int(count)
    }
    func snapshot() -> (Double?, Int, Int) {
        lock.lock(); defer { lock.unlock() }
        return (first, frames, routeChanges)
    }
}
private let receive: @convention(c) (UnsafePointer<Float>?, UInt32, UnsafeMutableRawPointer?) -> Void = { data, count, context in
    Unmanaged<CaptureCheck>.fromOpaque(context!).takeUnretainedValue().receive(data, count)
}

@main struct UtilHelperCheck {
    static func main() throws {
        precondition(microphoneAuthorizationStatus() == 3, "Grant microphone access before running this device check")
        let began = ProcessInfo.processInfo.systemUptime
        assert(!isCommunicationAudio("com.tencent.xinWeChat", input: false, output: false))
        assert(isCommunicationAudio("com.tencent.xinWeChat", input: true, output: false))
        assert(isCommunicationAudio("com.tencent.meeting", input: false, output: true))
        assert(isCommunicationAudio("com.tencent.flue.WeChatAppEx", input: true, output: false))
        assert(isCommunicationAudio("com.tencent.flue.helper.plugin", input: false, output: true, ownerPath: "/Applications/WeChat.app/Contents/Helpers/Plugin.app"))
        assert(!isCommunicationAudio("com.tencent.flue.helper.plugin", input: false, output: true, ownerPath: "/Applications/QQMusic.app/Contents/Helpers/Plugin.app"))
        assert(!isCommunicationAudio("com.tencent.QQMusicMac", input: false, output: true))
        assert(!isCommunicationAudio("com.tencent.meeting-other", input: true, output: true))
        assert(!isCommunicationAudio("com.ai-quick-input.assistant", input: true, output: false))
        // Explicitly prepare here to exercise the call path without making a call.
        try VoiceEngineCache.shared.warm()
        print("preparation_ms=\((ProcessInfo.processInfo.systemUptime - began) * 1000)")
        let prepared = try VoiceEngineCache.shared.take()
        assert(!prepared.engine.isRunning, "Preparation must not run the microphone")
        assert(prepared.engine.inputNode.isVoiceProcessingEnabled)
        assert(!prepared.engine.inputNode.isVoiceProcessingAGCEnabled)
        VoiceEngineCache.shared.store(prepared)

        for index in 1...4 {
            let check = CaptureCheck()
            var rate = 0.0
            guard let handle = startVoiceCapture(receive, Unmanaged.passUnretained(check).toOpaque(), &rate) else { fatalError("Capture failed") }
            let ready = ProcessInfo.processInfo.systemUptime
            try VoiceEngineCache.shared.warm() // Monitoring must not create a second graph while recording.
            Thread.sleep(forTimeInterval: 0.3)
            assert(check.snapshot().1 > 0 && rate > 0, "Every session must receive PCM")
            stopVoiceCapture(handle)
            let stopped = check.snapshot()
            Thread.sleep(forTimeInterval: 0.15)
            assert(check.snapshot().1 == stopped.1, "Stop must quiesce callbacks before Rust frees its context")
            let idle = try VoiceEngineCache.shared.take()
            assert(!idle.engine.isRunning, "Stop must release the active audio hardware")
            assert(idle === prepared, "Repeated sessions should reuse the stopped graph")
            VoiceEngineCache.shared.store(idle)
            print("attempt=\(index), start_ms=\((ready-check.began)*1000), first_frame_ms=\(((stopped.0 ?? ready)-check.began)*1000), rate=\(rate)")
        }

        // Simulate the same notification a route change sends while idle.
        NotificationCenter.default.post(name: .AVAudioEngineConfigurationChange, object: prepared.engine)
        let refreshed = try VoiceEngineCache.shared.take()
        assert(refreshed !== prepared && refreshed.reusable, "Do not reuse a graph invalidated by a device change")
        VoiceEngineCache.shared.store(refreshed)
        let check = CaptureCheck()
        var rate = 0.0
        guard let handle = startVoiceCapture(receive, Unmanaged.passUnretained(check).toOpaque(), &rate) else { fatalError("Capture after route change failed") }
        NotificationCenter.default.post(name: .AVAudioEngineConfigurationChange, object: refreshed.engine)
        assert(check.snapshot().2 == 1, "An active route change must still reach the Rust monitor")
        stopVoiceCapture(handle)
        let replacement = try VoiceEngineCache.shared.take()
        assert(replacement !== refreshed && !replacement.engine.isRunning)
        VoiceEngineCache.shared.store(replacement)
        print("Prepared capture, PCM delivery, stopped callbacks, and route invalidation passed")
    }
}
