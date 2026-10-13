import Foundation
import ApplicationServices
import AppKit
import AVFoundation
import CoreAudio

@_cdecl("request_accessibility_permission")
public func requestAccessibilityPermission() -> Bool {
    let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue(): true] as CFDictionary
    return AXIsProcessTrustedWithOptions(options)
}

// MARK: - Audio Control

/// Returns the current microphone authorization status.
/// 0 = not determined, 1 = restricted, 2 = denied, 3 = authorized.
@_cdecl("microphone_authorization_status")
public func microphoneAuthorizationStatus() -> Int32 {
    return Int32(AVCaptureDevice.authorizationStatus(for: .audio).rawValue)
}

/// Starts the microphone permission request without blocking the caller.
/// The system prompt is dispatched on the main queue; Rust polls the status
/// asynchronously before it opens the CoreAudio input stream.
@_cdecl("request_microphone_permission")
public func requestMicrophonePermission() -> Bool {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized:
        return true
    case .denied, .restricted:
        return false
    case .notDetermined:
        DispatchQueue.main.async {
            AVCaptureDevice.requestAccess(for: .audio) { _ in }
        }
        return true
    @unknown default:
        return false
    }
}


@_cdecl("check_accessibility_permission")
public func checkAccessibilityPermission() -> Bool { AXIsProcessTrusted() }

private func audioUInt32(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                         scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> UInt32? {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    var value: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr ? value : nil
}

private func audioObjects(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                          scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> [AudioObjectID] {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    guard AudioObjectGetPropertyDataSize(object, &address, 0, nil, &size) == noErr, size > 0 else { return [] }
    var values = [AudioObjectID](repeating: 0, count: Int(size) / MemoryLayout<AudioObjectID>.size)
    guard values.withUnsafeMutableBytes({ AudioObjectGetPropertyData(object, &address, 0, nil, &size, $0.baseAddress!) }) == noErr else { return [] }
    return Array(values.prefix(Int(size) / MemoryLayout<AudioObjectID>.size))
}

func microphoneKind(transport: UInt32, inputTerminals: [UInt32]) -> (builtIn: Bool, headset: Bool) {
    let headset = inputTerminals.contains(kAudioStreamTerminalTypeHeadsetMicrophone)
        || transport == kAudioDeviceTransportTypeBluetooth || transport == kAudioDeviceTransportTypeBluetoothLE
    return (transport == kAudioDeviceTransportTypeBuiltIn && !headset, headset)
}

@_cdecl("microphone_devices_json")
public func microphoneDevicesJSON(_ destination: UnsafeMutablePointer<CChar>?, _ capacity: UInt32) -> UInt32 {
    let system = AudioObjectID(kAudioObjectSystemObject)
    let defaultInput = audioUInt32(system, kAudioHardwarePropertyDefaultInputDevice)
    let defaultOutput = audioUInt32(system, kAudioHardwarePropertyDefaultOutputDevice)
    let devices: [[String: Any]] = audioObjects(system, kAudioHardwarePropertyDevices).compactMap { device in
        let input = audioObjects(device, kAudioDevicePropertyStreams, scope: kAudioObjectPropertyScopeInput)
        guard !input.isEmpty, audioUInt32(device, kAudioDevicePropertyDeviceIsAlive) == 1 else { return nil }
        var address = AudioObjectPropertyAddress(mSelector: kAudioObjectPropertyName, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        var value: Unmanaged<CFString>?, size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        guard AudioObjectGetPropertyData(device, &address, 0, nil, &size, &value) == noErr, let value else { return nil }
        let kind = microphoneKind(transport: audioUInt32(device, kAudioDevicePropertyTransportType) ?? 0,
            inputTerminals: input.compactMap { audioUInt32($0, kAudioStreamPropertyTerminalType) })
        return ["name": value.takeRetainedValue() as String, "builtIn": kind.builtIn, "headset": kind.headset,
                "defaultInput": device == defaultInput, "defaultOutput": device == defaultOutput]
    }
    guard let data = try? JSONSerialization.data(withJSONObject: devices) else { return 0 }
    let required = UInt32(data.count + 1)
    guard let destination, capacity >= required else { return required }
    data.withUnsafeBytes { bytes in _ = memcpy(destination, bytes.baseAddress!, data.count) }
    destination[data.count] = 0
    return required
}

// Opening a chat app alone must not enable echo processing. These are the
// communication apps and the WeChat audio helper observed on this machine.
func isCommunicationAudio(_ bundle: String, input: Bool, output: Bool, ownerPath: String? = nil) -> Bool {
    let roots = ["com.tencent.xinWeChat", "com.tencent.meeting", "com.tencent.flue.WeChatAppEx"]
    let known = roots.contains { bundle == $0 || bundle.hasPrefix($0 + ".") }
    let wechatHelper = bundle == "com.tencent.flue.helper.plugin" && ownerPath?.contains("/WeChat.app/") == true
    return (input || output) && (known || wechatHelper)
}

private func audioProcessFlag(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector) -> Bool {
    var address = AudioObjectPropertyAddress(mSelector: selector, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var value: UInt32 = 0, size = UInt32(MemoryLayout<UInt32>.size)
    return AudioObjectGetPropertyData(object, &address, 0, nil, &size, &value) == noErr && value != 0
}

func detectCommunicationAudio() -> Bool {
    guard #available(macOS 14.2, *) else { return false }
    var address = AudioObjectPropertyAddress(mSelector: kAudioHardwarePropertyProcessObjectList, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
    var size: UInt32 = 0
    let system = AudioObjectID(kAudioObjectSystemObject)
    guard AudioObjectGetPropertyDataSize(system, &address, 0, nil, &size) == noErr, size > 0 else { return false }
    var objects = Array(repeating: AudioObjectID(0), count: Int(size) / MemoryLayout<AudioObjectID>.size)
    guard objects.withUnsafeMutableBytes({ AudioObjectGetPropertyData(system, &address, 0, nil, &size, $0.baseAddress!) }) == noErr else { return false }
    for object in objects.prefix(Int(size) / MemoryLayout<AudioObjectID>.size) {
        var bundle: Unmanaged<CFString>?
        var bytes = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        var property = AudioObjectPropertyAddress(mSelector: kAudioProcessPropertyBundleID, mScope: kAudioObjectPropertyScopeGlobal, mElement: kAudioObjectPropertyElementMain)
        guard AudioObjectGetPropertyData(object, &property, 0, nil, &bytes, &bundle) == noErr, let bundle else { continue }
        let name = bundle.takeRetainedValue() as String
        let known = isCommunicationAudio(name, input: true, output: false)
        guard known || name == "com.tencent.flue.helper.plugin" else { continue }
        let input = audioProcessFlag(object, kAudioProcessPropertyIsRunningInput)
        let output = audioProcessFlag(object, kAudioProcessPropertyIsRunningOutput)
        guard input || output else { continue }
        if known { return true }
        // Generic Tencent helpers may belong to a different app. Resolve only
        // active helper ownership, rather than treating every Tencent process as a call.
        var pid: pid_t = 0
        var pidSize = UInt32(MemoryLayout<pid_t>.size)
        property.mSelector = kAudioProcessPropertyPID
        if AudioObjectGetPropertyData(object, &property, 0, nil, &pidSize, &pid) == noErr,
           isCommunicationAudio(name, input: input, output: output,
              ownerPath: NSRunningApplication(processIdentifier: pid)?.bundleURL?.path) { return true }
    }
    return false
}

private final class CommunicationAudioMonitor {
    static let shared = CommunicationAudioMonitor()
    private let lock = NSLock()
    private var active = false
    private let timer: DispatchSourceTimer
    var needsProcessing: Bool { lock.lock(); defer { lock.unlock() }; return active }
    private init() {
        timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "QuietType.communication-audio", qos: .utility))
        timer.schedule(deadline: .now(), repeating: .milliseconds(500), leeway: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            let current = detectCommunicationAudio()
            self.lock.lock(); self.active = current; self.lock.unlock()
            if current && microphoneAuthorizationStatus() == 3 {
                do { try VoiceEngineCache.shared.warm() }
                catch { NSLog("QuietType call audio preparation deferred: %@", error.localizedDescription) }
            }
        }
        timer.resume()
    }
}

@_cdecl("communication_audio_active")
public func communicationAudioActive() -> Bool {
    // The hotkey path only reads a cached flag; process checks run off-thread.
    CommunicationAudioMonitor.shared.needsProcessing
}

// macOS VoiceProcessingIO suppresses speaker leakage into the microphone
// while other apps keep playing. Its effectiveness depends on the audio route.
private typealias AudioSamplesCallback = @convention(c) (UnsafePointer<Float>?, UInt32, UnsafeMutableRawPointer?) -> Void
// Keep only a stopped, configured graph. Never run the microphone to warm it.
final class PreparedVoiceEngine {
    let engine = AVAudioEngine()
    private let lock = NSLock()
    private var valid = true
    private var observer: NSObjectProtocol?
    var reusable: Bool { lock.lock(); defer { lock.unlock() }; return valid }
    init() throws {
        let input = engine.inputNode
        try input.setVoiceProcessingEnabled(true)
        input.isVoiceProcessingAGCEnabled = false
        if #available(macOS 14.0, *) {
            input.voiceProcessingOtherAudioDuckingConfiguration = .init(enableAdvancedDucking: false, duckingLevel: .min)
        }
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.lock.lock(); self.valid = false; self.lock.unlock()
        }
    }
    deinit { if let observer { NotificationCenter.default.removeObserver(observer) } }
}

final class VoiceEngineCache {
    static let shared = VoiceEngineCache()
    private let lock = NSLock()
    private var idle: PreparedVoiceEngine?
    private var inUse = false
    func take() throws -> PreparedVoiceEngine {
        lock.lock(); defer { lock.unlock() }
        let prepared = idle; idle = nil
        let result: PreparedVoiceEngine
        if let prepared, prepared.reusable { result = prepared }
        else { result = try PreparedVoiceEngine() }
        inUse = true
        return result
    }
    func store(_ prepared: PreparedVoiceEngine, reuse: Bool = true) {
        lock.lock(); defer { lock.unlock() }
        inUse = false
        if reuse && prepared.reusable && !prepared.engine.isRunning && idle == nil { idle = prepared }
    }
    func warm() throws {
        lock.lock(); defer { lock.unlock() }
        if !inUse && idle?.reusable != true { idle = try PreparedVoiceEngine() }
    }
}

@_cdecl("prepare_voice_capture")
public func prepareVoiceCapture() {
    // Monitoring is cheap and never opens an input device. Only call audio
    // activity triggers graph preparation, never ordinary dictation.
    _ = CommunicationAudioMonitor.shared
}

private final class VoiceCapture {
    let prepared: PreparedVoiceEngine
    var engine: AVAudioEngine { prepared.engine }
    let lock = NSLock()
    var active = false
    var tapped = false
    var stopped = false
    var started = false
    var observer: NSObjectProtocol?
    let callback: AudioSamplesCallback
    let context: UnsafeMutableRawPointer?
    init(callback: @escaping AudioSamplesCallback, context: UnsafeMutableRawPointer?) throws {
        prepared = try VoiceEngineCache.shared.take()
        self.callback = callback; self.context = context
    }
    func start() throws -> Double {
        let input = engine.inputNode
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else {
            throw NSError(domain: "QuietTypeAudio", code: 1)
        }
        active = true
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { [weak self] buffer, _ in
            guard let self else { return }
            self.lock.lock(); defer { self.lock.unlock() }
            guard self.active, let data = buffer.floatChannelData else { return }
            self.callback(data[0], buffer.frameLength, self.context)
        }
        tapped = true
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil) { [weak self] _ in
            guard let self else { return }
            self.lock.lock(); defer { self.lock.unlock() }
            if self.active { self.callback(nil, 0, self.context) }
        }
        try engine.start()
        started = true
        return format.sampleRate
    }
    func stop() {
        lock.lock()
        if stopped { lock.unlock(); return }
        stopped = true; active = false
        lock.unlock()
        if let observer { NotificationCenter.default.removeObserver(observer) }
        observer = nil
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus: 0); tapped = false }
        VoiceEngineCache.shared.store(prepared, reuse: started)
    }
    deinit { stop() }
}

@_cdecl("start_voice_capture")
public func startVoiceCapture(_ callback: @escaping @convention(c) (UnsafePointer<Float>?, UInt32, UnsafeMutableRawPointer?) -> Void,
                              _ context: UnsafeMutableRawPointer?, _ sampleRate: UnsafeMutablePointer<Double>) -> UnsafeMutableRawPointer? {
    var capture: VoiceCapture?
    do {
        let current = try VoiceCapture(callback: callback, context: context)
        capture = current
        sampleRate.pointee = try current.start()
        return Unmanaged.passRetained(current).toOpaque()
    } catch {
        NSLog("QuietType voice processing unavailable: %@", error.localizedDescription)
        capture?.stop()
        return nil
    }
}

@_cdecl("stop_voice_capture")
public func stopVoiceCapture(_ handle: UnsafeMutableRawPointer) {
    let capture = Unmanaged<VoiceCapture>.fromOpaque(handle).takeRetainedValue()
    capture.stop()
}
