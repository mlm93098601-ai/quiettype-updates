import AppKit
import Combine

@main struct NativeUICheck {
    static func main() {
        var gesture = FnGesture()
        for _ in 0..<3 {
            assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function) == .press(english: false),
                   "Fn must request capture on key down, before the user releases it")
            assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function) == nil,
                   "A repeated down must not toggle the same recording off")
            assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == .release)
            assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == nil)
        }
        _ = gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function)
        assert(gesture.handle(type: .keyDown, keyCode: 122, flags: .function) == .cancel,
               "Fn plus an ordinary key must discard the just-started capture")
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == nil)
        var hud = HUDVisibility()
        hud.show()
        let oldEntrance = hud.revision
        assert(hud.visible && !hud.revealed, "The initial frame must remain small before reveal")
        assert(hud.reveal(oldEntrance) && hud.revealed)
        let oldHide = hud.hide()!
        assert(!hud.reveal(oldEntrance), "A cancelled entrance must not resurrect the HUD")
        assert(hud.hide() == nil)
        hud.show()
        assert(!hud.reveal(oldEntrance), "A stale entrance must not reveal the next session")
        assert(hud.reveal(hud.revision))
        assert(!hud.canFinishHide(oldHide), "A stale exit must not hide the next recording")
        let currentHide = hud.hide()!
        assert(hud.canFinishHide(currentHide))
        var meter = MicrophoneMeter()
        meter.update([0, 0.2, 1, 0.7, 0.1, 0, 0, 0, 0, 0, 0])
        assert(meter.levels[0] >= 0 && meter.levels[0] < meter.levels[1] && meter.levels[2] > meter.levels[3])
        let peak = meter.levels[2]
        meter.update(Array(repeating: 0, count: 11))
        assert(meter.levels[2] > 0 && meter.levels[2] < peak)
        meter.update([.nan, .infinity, -1, 2, 0, 0, 0, 0, 0, 0, 0])
        assert(meter.levels.allSatisfy { $0.isFinite && (0...1).contains($0) })
        var quietMeter = MicrophoneMeter()
        quietMeter.update(Array(repeating: 0.4, count: 11))
        assert(3 + quietMeter.levels[5] * 17 > 8, "Quiet speech must visibly rise above the 3px baseline")
        for _ in 0..<6 { quietMeter.update(Array(repeating: 0, count: 11)) }
        assert(quietMeter.levels[5] < 0.04, "A short pause must visibly return near baseline")
        var speechMeter = MicrophoneMeter()
        speechMeter.update([0.2, 1, 0.15, 0.8, 0.1, 1, 0.18, 0.65, 0.12, 0.9, 0.25])
        assert(zip(speechMeter.levels, speechMeter.levels.dropFirst()).filter { abs($0 - $1) * 17 > 5 }.count >= 8,
               "Neighbouring speech bars must retain more than 5px of height contrast")
        _ = gesture.handle(type: .flagsChanged, keyCode: 63, flags: [.function, .command])
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == nil)
        // Both modifier orders start immediately; releasing Shift must retain English.
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: [.function, .shift]) == .press(english: true))
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: .shift) == .release)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function) == .press(english: false))
        assert(gesture.handle(type: .flagsChanged, keyCode: 56, flags: [.function, .shift]) == .english)
        assert(gesture.handle(type: .flagsChanged, keyCode: 56, flags: .function) == nil)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == .release)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: [.function, .shift]) == .press(english: true))
        assert(gesture.handle(type: .keyDown, keyCode: 0, flags: [.function, .shift]) == .cancel)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == nil)
        _ = gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function)
        assert(gesture.handle(type: .flagsChanged, keyCode: 55, flags: [.function, .command]) == .cancel)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: []) == nil)
        // The same ordinaryEnabled gate used by handleFn must observe Fn before Shift.
        _ = gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function, ordinaryEnabled: false)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: [], ordinaryEnabled: false) == nil)
        _ = gesture.handle(type: .flagsChanged, keyCode: 63, flags: .function, ordinaryEnabled: false)
        assert(gesture.handle(type: .flagsChanged, keyCode: 56, flags: [.function, .shift], ordinaryEnabled: false) == .press(english: true))
        assert(gesture.handle(type: .flagsChanged, keyCode: 56, flags: .function, ordinaryEnabled: false) == nil)
        assert(gesture.handle(type: .flagsChanged, keyCode: 63, flags: [], ordinaryEnabled: false) == .release)
        let model = InterfaceState()
        model.wordDraft = "QuietType"; model.triggerDraft = "我的邮箱"; model.expansionDraft = "name@example.com\n"
        model.didSavePersonal("delete_word")
        assert(model.wordDraft == "QuietType" && model.triggerDraft == "我的邮箱")
        model.didSavePersonal("add_word")
        assert(model.wordDraft.isEmpty && model.triggerDraft == "我的邮箱" && model.expansionDraft == "name@example.com\n")
        model.wordDraft = "模型"; model.didSavePersonal("add_snippet")
        assert(model.wordDraft == "模型" && model.triggerDraft.isEmpty && model.expansionDraft.isEmpty)
        model.phase = "inserting"
        assert(model.active && model.busy, "Delayed insertion must block a second recording")
        func apply(_ json: String) { InterfaceUpdate(json)!.apply(to: model) }
        assert(InterfaceUpdate("invalid") == nil)
        var interfaceChanges = 0, meterChanges = 0
        let interfaceSubscription = model.objectWillChange.sink { interfaceChanges += 1 }
        let meterSubscription = model.meter.objectWillChange.sink { meterChanges += 1 }
        model.meter.update(Array(repeating: 1, count: 11))
        apply(#"{"voice":{"sessionId":7,"phase":"starting","status":"准备麦克风"}}"#)
        assert(model.phase == "starting" && model.hudVoice["phase"] as? String == "starting")
        assert(model.meter.levels.allSatisfy { $0 == 0 }, "Starting must clear the previous waveform")
        interfaceChanges = 0; meterChanges = 0
        apply(#"{"sessionId":7,"audioLevels":[1,1,1,1,1,1,1,1,1,1,1]}"#)
        assert(interfaceChanges == 0 && meterChanges == 0, "Starting must not display audio before recording is ready")
        // Simulate the already shown panel's visibility without opening a window.
        model.hud.show(); model.hud.reveal(model.hud.revision)
        let entrance = model.hud.revision
        apply(#"{"voice":{"sessionId":7,"phase":"recording","status":"录音中","elapsedSeconds":0,"limitWarning":false}}"#)
        assert(model.hud.visible && model.hud.revealed && model.hud.revision == entrance,
               "Starting to recording must retain the same visible HUD")
        assert(model.hudVoice["phase"] as? String == "recording")
        interfaceChanges = 0; meterChanges = 0
        for _ in 0..<20 { apply(#"{"sessionId":7,"audioLevels":[0,0.2,1,0.7,0.1,0,0,0,0,0,0]}"#) }
        assert(interfaceChanges == 0 && meterChanges == 20, "20 Hz audio must notify only the waveform")
        assert(model.meter.levels[2] > model.meter.levels[3])
        meterChanges = 0
        apply(#"{"sessionId":6,"audioLevels":[1,1,1,1,1,1,1,1,1,1,1]}"#)
        assert(interfaceChanges == 0 && meterChanges == 0, "An old session must not change the waveform")
        apply(#"{"voice":{"sessionId":7,"phase":"recording","status":"录音中","elapsedSeconds":1,"limitWarning":false}}"#)
        assert(interfaceChanges == 1 && meterChanges == 0 && model.elapsedSeconds == 1,
               "A clock tick must publish only elapsedSeconds, not nine unchanged fields")
        interfaceChanges = 0
        apply(#"{"voice":{"sessionId":7,"phase":"recording","status":"录音中","elapsedSeconds":1,"limitWarning":false}}"#)
        assert(interfaceChanges == 0, "An identical clock snapshot must not publish")
        apply(#"{"voice":{"sessionId":7,"phase":"recording","elapsedSeconds":270,"limitWarning":true}}"#)
        assert(interfaceChanges == 3 && model.limitWarning && model.hudVoice["limitWarning"] as? Bool == true,
               "The 4:30 tick must update elapsedSeconds and both warning consumers")
        interfaceChanges = 0
        apply(#"{"voice":{"sessionId":7,"phase":"recording","elapsedSeconds":271,"limitWarning":true}}"#)
        assert(interfaceChanges == 1 && model.status == "录音中" && model.limitWarning)
        interfaceChanges = 0
        apply(#"{"voice":{"sessionId":6,"phase":"recording","elapsedSeconds":0,"limitWarning":false}}"#)
        assert(interfaceChanges == 0 && model.elapsedSeconds == 271 && model.limitWarning)
        apply(#"{"voice":{"sessionId":7,"phase":"error","status":"请重试","error":"设备中断","canRetry":true,"canInsert":false,"elapsedSeconds":271,"limitWarning":false}}"#)
        assert(model.phase == "error" && model.error == "设备中断" && model.canRetry && !model.limitWarning)
        apply(#"{"voice":{"sessionId":8,"phase":"recording","status":"新会话","canInsert":true,"injected":true,"elapsedSeconds":0,"limitWarning":false}}"#)
        assert(model.sessionId == 8 && model.status == "新会话" && model.canInsert && model.injected)
        assert(model.meter.levels.allSatisfy { $0 == 0 } && model.elapsedSeconds == 0 && !model.limitWarning)
        apply(#"{"voice":{"sessionId":9,"phase":"recording","status":"另一个会话","elapsedSeconds":0}}"#)
        assert(model.sessionId == 9 && model.status == "另一个会话" && !model.canInsert && !model.injected,
               "A new session with the same recording phase must still apply its complete snapshot")
        apply(#"{"accessibilityGranted":true,"microphoneStatus":3,"firstInputSent":true,"runtime":{"phase":"ready","message":"就绪","ready":true}}"#)
        assert(model.accessibilityGranted && model.microphoneStatus == 3 && model.firstInputSent && model.runtimeReady)
        apply(#"{"settings":{"shortcutRecord":"Fn"}}"#)
        assert(model.microphoneSource == "system", "Existing settings must keep the system microphone default")
        for source in ["system", "builtIn", "headset"] {
            apply("{\"settings\":{\"microphoneSource\":\"\(source)\"}}")
            assert(model.microphoneSource == source, "The picker must reflect the saved microphone source")
        }
        withExtendedLifetime((interfaceSubscription, meterSubscription)) {}
        let firstInstall = InterfaceState()
        InterfaceUpdate(#"{"runtime":{"phase":"downloading","message":"正在下载本地语音，35%","ready":false}}"#)!.apply(to: firstInstall)
        assert(!firstInstall.runtimeReady && firstInstall.actionTitle == "语音准备中…" && firstInstall.runtimeMessage.contains("35%"))
        InterfaceUpdate(#"{"runtime":{"phase":"ready","message":"本地语音已就绪","ready":true}}"#)!.apply(to: firstInstall)
        assert(firstInstall.actionTitle == "开始说话")
        print("Fn, drafts, insertion, HUD continuity, microphone smoothing, isolated audio, clock ticks, warnings, and setup updates passed")
    }
}
