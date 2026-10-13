import AppKit
import SwiftUI
import AVFoundation

typealias ActionCallback = @convention(c) (UnsafePointer<CChar>) -> Void
private var callback: ActionCallback?
private let state = InterfaceState()
private var mainWindow: NSWindow?
private var recordingPanel: NSPanel?
private var shortcutMonitor: Any?
private var fnLocalMonitor: Any?
private var fnGlobalMonitor: Any?
private var fnEventTap: CFMachPort?
private var fnTapSource: CFRunLoopSource?
private var fnTapTimer: Timer?
private var fnGesture = FnGesture()
private var startSound: NSSound?

struct HUDVisibility {
    private(set) var visible = false
    private(set) var revealed = false
    private(set) var revision: UInt64 = 0
    mutating func show() { revision &+= 1; visible = true; revealed = false }
    @discardableResult mutating func reveal(_ token: UInt64) -> Bool {
        guard visible && token == revision else { return false }
        revealed = true; return true
    }
    mutating func hide() -> UInt64? {
        guard visible else { return nil }
        revision &+= 1; visible = false; revealed = false; return revision
    }
    func canFinishHide(_ token: UInt64) -> Bool { !visible && token == revision }
}

struct MicrophoneMeter {
    private(set) var levels = [CGFloat](repeating: 0, count: 11)
    mutating func update(_ samples: [Double]) {
        guard samples.count == levels.count else { return }
        let clean = samples.map { $0.isFinite ? CGFloat(min(1, max(0, $0))) : 0 }
        levels = levels.enumerated().map { index, previous in
            // Preserve neighbouring syllable peaks; smooth mostly over time.
            let target = 0.94 * clean[index] + 0.03 * clean[max(0, index - 1)] + 0.03 * clean[min(clean.count - 1, index + 1)]
            return previous + (target - previous) * (target > previous ? 0.90 : 0.50)
        }
    }
}

final class MicrophoneState: ObservableObject {
    @Published private(set) var meter = MicrophoneMeter()
    var levels: [CGFloat] { meter.levels }
    func update(_ samples: [Double]) { meter.update(samples) }
    func reset() { meter = MicrophoneMeter() }
}

private func playStartSound(_ choice: String) {
    let name = ["soft", "droplet", "tap"].contains(choice) ? choice : "soft"
    guard let url = Bundle.main.resourceURL?.appendingPathComponent("sounds/\(name).wav"),
          let sound = NSSound(contentsOf: url, byReference: false) else { return }
    startSound?.stop(); startSound = sound; sound.play()
}

private func showHUD(for model: InterfaceState = state) {
    guard let panel = recordingPanel, !model.hud.visible else { return }
    if !panel.isVisible, let screen = NSScreen.screens.first(where: { NSMouseInRect(NSEvent.mouseLocation, $0.frame, false) }) ?? NSScreen.main {
        panel.setFrameOrigin(NSPoint(x: screen.visibleFrame.midX - 86, y: screen.visibleFrame.minY + 10))
    }
    // Render the small, transparent starting frame before revealing it.
    // Otherwise the first window paint can coalesce both states and skip the entrance.
    model.hud.show()
    let token = model.hud.revision
    panel.ignoresMouseEvents = false
    panel.orderFrontRegardless()
    panel.contentView?.layoutSubtreeIfNeeded(); panel.contentView?.displayIfNeeded()
    DispatchQueue.main.asyncAfter(deadline: .now() + 1.0 / 60.0) {
        let reduced = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        withAnimation(reduced ? .linear(duration: 0.1) : .spring(response: 0.34, dampingFraction: 0.9)) {
            _ = model.hud.reveal(token)
        }
    }
}

private func hideHUD(for model: InterfaceState = state) {
    let duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1 : 0.2
    guard let token = withAnimation(.easeIn(duration: duration), { model.hud.hide() }) else { return }
    recordingPanel?.ignoresMouseEvents = true
    DispatchQueue.main.asyncAfter(deadline: .now() + duration) {
        if model.hud.canFinishHide(token) { recordingPanel?.orderOut(nil) }
    }
}

struct FnGesture {
    enum Action: Equatable {
        case press(english: Bool), english, cancel, release
    }
    private var armed: Bool?
    private var started = false
    mutating func handle(type: NSEvent.EventType, keyCode: UInt16, flags: NSEvent.ModifierFlags, ordinaryEnabled: Bool = true) -> Action? {
        if type == .flagsChanged, [56, 60].contains(keyCode), armed != nil {
            if flags.contains(.shift), armed == false {
                armed = true
                let action: Action = started ? .english : .press(english: true)
                started = true
                return action
            }
            return nil
        }
        guard type == .flagsChanged, keyCode == 63 else {
            let cancel = started
            self = FnGesture()
            return cancel ? .cancel : nil
        }
        if flags.contains(.function) {
            if armed == nil, flags.intersection([.command, .control, .option]).isEmpty {
                armed = flags.contains(.shift)
                started = ordinaryEnabled || flags.contains(.shift)
                if started { return .press(english: flags.contains(.shift)) }
            }
            return nil
        }
        let release = started
        self = FnGesture()
        return release ? .release : nil
    }
}

private func handleFn(_ event: NSEvent) -> Bool {
    // Shift+Fn remains available with a customized ordinary recording shortcut.
    if let action = fnGesture.handle(type: event.type, keyCode: event.keyCode, flags: event.modifierFlags, ordinaryEnabled: state.capturing || state.shortcut == "Fn") {
        if state.capturing {
            if case .press = action { state.stopCapture(); send("settings", ["shortcutRecord": "Fn"]) }
        } else {
            switch action {
            case .press(let english): send("fnPress", ["english": english])
            case .english: send("fnEnglish")
            case .cancel: send("fnCancel")
            case .release: send("fnRelease")
            }
        }
    }
    return event.type == .flagsChanged && event.keyCode == 63
}
private func installFnEventTap() {
    guard fnEventTap == nil, AXIsProcessTrusted() else { return }
    let mask = (CGEventMask(1) << CGEventType.flagsChanged.rawValue) | (CGEventMask(1) << CGEventType.keyDown.rawValue)
    guard let tap = CGEvent.tapCreate(tap: .cgSessionEventTap, place: .headInsertEventTap,
                                     options: .defaultTap, eventsOfInterest: mask,
                                     callback: { _, type, event, _ in
        if type == .tapDisabledByTimeout || type == .tapDisabledByUserInput {
            send("fnCancel")
            fnGesture = FnGesture()
            if let tap = fnEventTap { CGEvent.tapEnable(tap: tap, enable: true) }
        } else if let native = NSEvent(cgEvent: event) { _ = handleFn(native) }
        // Observe before application dispatch, but preserve every original key.
        return Unmanaged.passUnretained(event)
    }, userInfo: nil), let source = CFMachPortCreateRunLoopSource(kCFAllocatorDefault, tap, 0) else { return }
    fnGesture = FnGesture()
    fnEventTap = tap; fnTapSource = source
    CFRunLoopAddSource(CFRunLoopGetMain(), source, .commonModes)
    CGEvent.tapEnable(tap: tap, enable: true)
}
private let delegate = WindowDelegate()
private final class HostingView<Content: View>: NSHostingView<Content> {
    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
}
private final class RecordingPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

private func send(_ action: String, _ args: [String: Any] = [:]) {
    guard let data = try? JSONSerialization.data(withJSONObject: ["action": action, "args": args]),
          let json = String(data: data, encoding: .utf8) else { return }
    json.withCString { callback?($0) }
}
@discardableResult private func copy(_ text: String) -> Bool {
    guard !text.isEmpty else { return false }
    NSPasteboard.general.clearContents()
    guard NSPasteboard.general.setString(text, forType: .string) else {
        state.notice = "复制失败，请重试"
        return false
    }
    state.notice = "已复制"
    return true
}
private struct CopyButton: View {
    let text: String
    var onCopied: (() -> Void)? = nil
    @State private var copied = false
    @State private var copyRevision: UInt64 = 0
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var body: some View {
        Button {
            guard copy(text) else { return }
            copyRevision &+= 1
            let revision = copyRevision
            copied = true
            onCopied?()
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) {
                if copyRevision == revision { copied = false }
            }
        } label: {
            Image(systemName: copied ? "checkmark" : "doc.on.doc")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(copied ? Color.secondary : Color.primary)
                .frame(width: 22, height: 22)
        }.buttonStyle(.plain).disabled(text.isEmpty || copied)
            .help(copied ? "已复制" : "复制完整文字")
            .accessibilityLabel(copied ? "已复制" : "复制完整文字")
            .animation(reduceMotion ? nil : .easeOut(duration: 0.12), value: copied)
            .onChange(of: text) { _ in copyRevision &+= 1; copied = false }
    }
}
final class InterfaceState: ObservableObject {
    @Published var phase = "idle"
    // Audio arrives at 20 Hz. Only the waveform observes this separate object.
    let meter = MicrophoneState()
    @Published var hud = HUDVisibility()
    @Published var hudVoice: [String: Any] = [:]
    var sessionId: UInt64 = 0
    @Published var transcript = ""
    @Published var status = ""
    @Published var error = ""
    @Published var canRetry = false
    @Published var injected = false
    @Published var canInsert = false
    @Published var elapsedSeconds = 0
    @Published var limitWarning = false
    @Published var notice = ""
    @Published var settings: [String: Any] = [:]
    var historyQuery = ""
    @Published var records: [[String: Any]] = []
    @Published var more = false
    @Published var historyBusy = false
    @Published var shortcutReady = true
    @Published var capturing = false
    @Published var accessibilityGranted = false
    @Published var microphoneStatus = 0
    @Published var runtimePhase = "checking"
    @Published var runtimeMessage = "正在检查本地语音资源"
    @Published var runtimeReady = false
    @Published var firstInputSent = false
    @Published var words: [[String: Any]] = []
    @Published var snippets: [[String: Any]] = []
    @Published var personalBusy = false
    @Published var wordDraft = ""
    @Published var triggerDraft = ""
    @Published var expansionDraft = ""
    func didSavePersonal(_ action: String) {
        if action == "add_word" { wordDraft = "" }
        if action == "add_snippet" { triggerDraft = ""; expansionDraft = "" }
    }
    var active: Bool { ["starting", "recording", "stopping", "transcribing", "retrying", "inserting"].contains(phase) }
    var busy: Bool { active && phase != "recording" }
    var actionTitle: String {
        switch phase {
        case "starting": return "准备录音…"
        case "recording": return "完成"
        case "stopping": return "结束录音…"
        case "inserting": return "准备填入…"
        default: return canRetry ? "重新识别保留录音" : busy ? "识别中…" : !runtimeReady ? runtimePhase == "error" ? "语音未就绪" : "语音准备中…" : "开始说话"
        }
    }
    var shortcut: String { settings["shortcutRecord"] as? String ?? "Fn" }
    var microphoneSource: String { settings["microphoneSource"] as? String ?? "system" }
    func stopCapture() {
        capturing = false
        send("shortcutCapture", ["active": false])
        if let monitor = shortcutMonitor { NSEvent.removeMonitor(monitor); shortcutMonitor = nil }
    }
    func captureShortcut() {
        stopCapture(); capturing = true
        send("shortcutCapture", ["active": true])
        shortcutMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { event in
            guard state.capturing, event.window === mainWindow else { return event }
            fnGesture = FnGesture()
            if event.keyCode == 53 { state.stopCapture(); return nil }
            let codes: [UInt16: String] = [0:"A",1:"S",2:"D",3:"F",4:"H",5:"G",6:"Z",7:"X",8:"C",9:"V",11:"B",12:"Q",13:"W",14:"E",15:"R",16:"Y",17:"T",18:"1",19:"2",20:"3",21:"4",22:"6",23:"5",25:"9",26:"7",28:"8",29:"0",31:"O",32:"U",34:"I",35:"P",37:"L",38:"J",40:"K",45:"N",46:"M",49:"Space"]
            guard let key = codes[event.keyCode] else { return nil }
            var parts: [String] = []
            if event.modifierFlags.contains(.command) { parts.append("⌘") }
            if event.modifierFlags.contains(.control) { parts.append("⌃") }
            if event.modifierFlags.contains(.option) { parts.append("⌥") }
            if event.modifierFlags.contains(.shift) { parts.append("⇧") }
            guard !parts.isEmpty else { state.notice = "请使用组合键"; return nil }
            parts.append(key); state.stopCapture()
            send("settings", ["shortcutRecord": parts.joined(separator: " ")])
            return nil
        }
    }
}
private func openSystemSettings(_ section: String) {
    let allowed = ["microphone": "com.apple.preference.security?Privacy_Microphone",
                   "accessibility": "com.apple.preference.security?Privacy_Accessibility",
                   "keyboard": "com.apple.Keyboard-Settings.extension"]
    guard let path = allowed[section], let url = URL(string: "x-apple.systempreferences:" + path) else { return }
    NSWorkspace.shared.open(url)
}
private struct SetupChecks: View {
    @ObservedObject var model: InterfaceState
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label(model.microphoneStatus == 3 ? "麦克风已允许" : "麦克风需要允许", systemImage: model.microphoneStatus == 3 ? "checkmark.circle" : "mic")
                Spacer()
                if model.microphoneStatus != 3 {
                    Button(model.microphoneStatus == 0 ? "允许麦克风" : "打开设置") {
                        if model.microphoneStatus == 0 { send("requestMicrophone") }
                        else { openSystemSettings("microphone") }
                    }
                }
            }
            HStack {
                Label(model.accessibilityGranted ? "辅助功能已允许" : "辅助功能需要允许", systemImage: model.accessibilityGranted ? "checkmark.circle" : "keyboard")
                Spacer()
                if !model.accessibilityGranted { Button("打开设置") { openSystemSettings("accessibility") } }
            }
            HStack {
                Label(model.runtimeMessage, systemImage: model.runtimeReady ? "checkmark.circle" : "waveform")
                Spacer()
                if model.runtimePhase == "error" { Button("重新准备") { send("prepare") }.disabled(model.active) }
                else if !model.runtimeReady { ProgressView().controlSize(.small) }
            }
            HStack {
                Text("Fn / 地球键设为“不执行任何操作”").font(.caption).foregroundStyle(.secondary)
                Spacer(); Button("键盘设置") { openSystemSettings("keyboard") }
            }
            if model.settings["setupComplete"] as? Bool != true {
                Text("在其他软件点入一个文本框，按 \(model.shortcut) 说一句话，再按 \(model.shortcut) 结束；确认文字出现后完成检查").font(.callout)
                if model.firstInputSent {
                    Button("确认文字已出现在输入框") { send("settings", ["setupComplete": true]) }
                }
            }
            Button("重新检查权限") { send("refresh") }.font(.caption)
        }.frame(maxWidth: .infinity, alignment: .leading)
    }
}
private struct PersonalizationView: View {
    @ObservedObject var model: InterfaceState
    @State private var showWords = false
    @State private var showSnippets = false
    @State private var pendingRemoval: [String: String]?
    @State private var confirmRemoval = false
    private func load() { model.personalBusy = true; send("personal") }
    private func change(_ args: [String: Any]) { model.personalBusy = true; send("personalChange", args) }
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            DisclosureGroup("常用词（\(model.words.count)）", isExpanded: $showWords) {
                VStack(alignment: .leading, spacing: 10) {
                    Text("添加人名、软件名或专业词的正确写法；每次识别最多参考 12 个词，不强行替换原话").font(.caption).foregroundStyle(.secondary)
                    HStack {
                        TextField("正确写法，例如 QuietType", text: $model.wordDraft)
                        Button("记住写法") { change(["action": "add_word", "word": model.wordDraft]) }
                            .disabled(model.personalBusy || model.wordDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                    if model.words.isEmpty && !model.personalBusy { Text("还没有常用词，自动学习的词也会显示在这里").font(.caption).foregroundStyle(.secondary) }
                    ForEach(model.words.indices, id: \.self) { index in
                        let row = model.words[index]
                        let value = row["word"] as? String ?? ""
                        HStack {
                            Text(value).textSelection(.enabled)
                            Spacer()
                            Text(row["confirmed"] as? Bool == true ? "手动" : "自动学习").font(.caption).foregroundStyle(.secondary)
                            Button { pendingRemoval = ["action": "delete_word", "word": value]; confirmRemoval = true } label: { Image(systemName: "trash") }
                                .help("删除常用词").accessibilityLabel("删除常用词 \(value)")
                        }
                    }
                    Text("删除后不再作为参考；以后多次说到，可能重新学习").font(.caption).foregroundStyle(.secondary)
                }.padding(.top, 8)
            }.onChange(of: showWords) { if $0 { load() } }
            DisclosureGroup("快捷短语（\(model.snippets.count)）", isExpanded: $showSnippets) {
                VStack(alignment: .leading, spacing: 10) {
                    Text("整句说出触发词即可填入预存文字；例如“我的邮箱”，只在普通听写生效，英文输出不展开").font(.caption).foregroundStyle(.secondary)
                    TextField("触发词，3–60 字，可用文字、数字和空格", text: $model.triggerDraft)
                    Text("展开文字，最多 4000 字").font(.caption).foregroundStyle(.secondary)
                    TextEditor(text: $model.expansionDraft).frame(minHeight: 70, maxHeight: 110).border(Color.secondary.opacity(0.25))
                    Button("添加短语") { change(["action": "add_snippet", "trigger": model.triggerDraft, "text": model.expansionDraft]) }
                        .disabled(model.personalBusy || model.triggerDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || model.expansionDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    if model.snippets.isEmpty && !model.personalBusy { Text("还没有快捷短语").font(.caption).foregroundStyle(.secondary) }
                    ForEach(model.snippets.indices, id: \.self) { index in
                        let row = model.snippets[index]
                        let value = row["trigger"] as? String ?? ""
                        VStack(alignment: .leading, spacing: 5) {
                            HStack {
                                Text(value).fontWeight(.medium); Spacer()
                                Button { pendingRemoval = ["action": "delete_snippet", "trigger": value]; confirmRemoval = true } label: { Image(systemName: "trash") }
                                    .help("删除快捷短语").accessibilityLabel("删除快捷短语 \(value)")
                            }
                            Text(row["text"] as? String ?? "").lineLimit(3).textSelection(.enabled).foregroundStyle(.secondary)
                        }
                    }
                }.padding(.top, 8)
            }.onChange(of: showSnippets) { if $0 { load() } }
            if model.personalBusy { ProgressView("正在读取或保存…").controlSize(.small) }
        }
        .disabled(model.personalBusy)
        .alert("删除这个词或短语？", isPresented: $confirmRemoval) {
            Button("取消", role: .cancel) {}
            Button("删除", role: .destructive) { if let args = pendingRemoval { change(args) } }
        } message: { Text("删除后可手动重新添加，其他词和短语不受影响。") }
    }
}
private final class WindowDelegate: NSObject, NSWindowDelegate {
    func windowDidBecomeKey(_ notification: Notification) { send("refresh") }
    func windowWillClose(_ notification: Notification) { state.stopCapture() }
}
private struct MainView: View {
    @ObservedObject var model: InterfaceState
    @State private var page = 0
    @State private var query = ""
    @State private var pendingDelete: String?
    @State private var confirmDeletion = false
    var body: some View {
        VStack(spacing: 0) {
            Picker("页面", selection: $page) { Text("输入").tag(0); Text("记录").tag(1); Text("设置").tag(2) }
                .pickerStyle(.segmented).labelsHidden().frame(width: 240).padding(.vertical, 20)
            Divider()
            if page == 0 { ScrollView { input.frame(maxWidth: .infinity) } }
            else if page == 1 { history }
            else { preferences }
            if !model.notice.isEmpty { Text(model.notice).font(.callout).foregroundStyle(.secondary).textSelection(.enabled).padding(12) }
        }
        .frame(minWidth: 520, minHeight: 420, alignment: .top).background(Color.white).preferredColorScheme(.light)
        .onChange(of: page) { value in model.stopCapture(); if value == 1 { send("history", ["query": query, "offset": 0]) } }
        .alert(pendingDelete == nil ? "清空全部记录？" : "删除这条记录？", isPresented: $confirmDeletion) {
            Button("取消", role: .cancel) {}
            Button("删除", role: .destructive) {
                if let id = pendingDelete { send("historyDelete", ["id": id]) }
                else { send("historyClear", ["confirmed": true]) }
            }
        } message: { Text("删除后无法恢复，其他设置和常用词不受影响。") }
    }
    private var input: some View {
        VStack(spacing: 18) {
            if model.settings["setupComplete"] as? Bool != true {
                GroupBox("首次使用") { SetupChecks(model: model).padding(6) }
            } else if !model.runtimeReady {
                SetupChecks(model: model)
            }
            Spacer(minLength: 16)
            Image(systemName: "mic").font(.system(size: 32, weight: .regular)).foregroundStyle(.black)
            HStack {
                Button(model.actionTitle) { send(model.canRetry ? "retry" : "toggle") }.disabled(model.busy || !model.runtimeReady && !model.canRetry)
                    .keyboardShortcut(.return, modifiers: []).controlSize(.large)
                if !model.active && !model.canRetry {
                    Button("英文输出") { send("toggleEnglish") }.disabled(!model.runtimeReady).controlSize(.large)
                }
                if model.canRetry { Button("重新录音") { send("toggle") } }
                if model.active || model.phase == "error" { Button("取消") { send("cancel") }.keyboardShortcut(.escape, modifiers: []) }
            }
            Text(model.active ? model.phase == "recording" ? "录音 \(model.elapsedSeconds / 60):\(String(format: "%02d", model.elapsedSeconds % 60)) / 5:00" : model.status : model.shortcut).font(.callout).foregroundStyle(.secondary)
            if !model.active { Text("\(model.shortcut) 听写 · Shift + Fn 输出英文；讲话期间可以切换输入框").font(.caption).foregroundStyle(.secondary) }
            if model.canRetry { Text("录音已保留，重新识别无需再说一次；重新录音会替换它").font(.caption).foregroundStyle(.secondary) }
            if model.limitWarning && model.phase == "recording" { Text("还剩不到 30 秒，即将自动结束本段录音").font(.callout) }
            if !model.error.isEmpty { Text(model.error).font(.callout).textSelection(.enabled) }
            if !model.shortcutReady { Text("快捷键不可用，请检查辅助功能权限或更换快捷键。").font(.callout) }
            Spacer(minLength: 16)
            Divider()
            HStack {
                Text("文字").foregroundStyle(.secondary); Spacer()
                if model.canInsert { Button("2 秒后再次填入") { send("reinsert", ["sessionId": model.sessionId, "delayMs": 2000]) }.help("点击后切换到目标输入框，无需重新录音") }
                CopyButton(text: model.transcript)
            }
            Text(model.transcript.isEmpty ? "文字会出现在这里" : model.transcript)
                .foregroundStyle(model.transcript.isEmpty ? Color.secondary : Color.primary).textSelection(.enabled)
                .frame(maxWidth: .infinity, minHeight: 120, alignment: .topLeading).padding(.vertical, 8)
            if !model.transcript.isEmpty && !model.status.isEmpty { Text(model.status).font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading) }
        }.padding(28)
    }
    private var history: some View {
        VStack(spacing: 12) {
            TextField("搜索记录", text: $query).textFieldStyle(.roundedBorder).onChange(of: query) { value in model.historyQuery = value; send("history", ["query": value, "offset": 0]) }
            HStack {
                Text("只保留最近 5 天，较早记录会自动删除").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button("清空记录", role: .destructive) { pendingDelete = nil; confirmDeletion = true }.disabled(model.records.isEmpty)
            }
            if model.historyBusy { ProgressView().controlSize(.small) }
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 18) {
                    if model.records.isEmpty && !model.historyBusy { Text(query.isEmpty ? "还没有记录" : "没有匹配记录").foregroundStyle(.secondary) }
                    ForEach(model.records.indices, id: \.self) { index in
                        let row = model.records[index]
                        let text = row["refinedText"] as? String ?? row["editedText"] as? String ?? ""
                        VStack(alignment: .leading, spacing: 8) {
                            HStack {
                                Text(dateLabel(row["createdAt"] as? String ?? "")).font(.caption).foregroundStyle(.secondary); Spacer(); CopyButton(text: text)
                                Button { pendingDelete = row["id"] as? String; if pendingDelete != nil { confirmDeletion = true } } label: { Image(systemName: "trash") }
                                    .buttonStyle(.plain).help("删除这条记录").accessibilityLabel("删除这条记录")
                            }
                            Text(text).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
                            Divider()
                        }
                    }
                    if model.more { Button("更多记录") { send("history", ["query": query, "offset": model.records.count]) }.disabled(model.historyBusy) }
                }.padding(.vertical, 8)
            }
        }.padding(24)
    }
    private var preferences: some View {
        Form {
            Section("使用检查") { SetupChecks(model: model) }
            Section("语音输入") {
            HStack {
                Text("开始 / 完成"); Spacer()
                Button(model.capturing ? "按下 Fn 或组合键…" : model.shortcut) { model.captureShortcut() }
                if model.shortcut != "Fn" { Button("使用 Fn") { model.stopCapture(); send("settings", ["shortcutRecord": "Fn"]) } }
            }
            Text("Esc 取消 · Enter 完成 · Shift + Fn 输出英文（自定义快捷键后仍可使用）").font(.caption).foregroundStyle(.secondary)
            Picker("麦克风来源", selection: Binding(get: { model.microphoneSource }, set: { value in
                guard !model.active else { return }
                send("settings", ["microphoneSource": value])
            })) { Text("系统默认").tag("system"); Text("电脑内置").tag("builtIn"); Text("耳机").tag("headset") }
                .disabled(model.active)
                .help(model.active ? "请结束本次语音输入后再切换麦克风" : "选择 QuietType 录音使用的麦克风")
            Text("仅影响 QuietType 录音，不改变播放设备；指定麦克风不可用时会提示，不自动换用其他麦克风").font(.caption).foregroundStyle(.secondary)
            Toggle("提示音", isOn: Binding(get: { model.settings["notifySound"] as? Bool ?? true }, set: { send("settings", ["notifySound": $0]) }))
            HStack {
                Picker("启动声音", selection: Binding(get: { model.settings["startSound"] as? String ?? "soft" }, set: { value in
                    playStartSound(value); send("settings", ["startSound": value])
                })) { Text("轻柔").tag("soft"); Text("水滴").tag("droplet"); Text("轻点").tag("tap") }
                Button { playStartSound(model.settings["startSound"] as? String ?? "soft") } label: { Label("试听", systemImage: "speaker.wave.2") }
            }
            Picker("识别语言", selection: Binding(get: { model.settings["sttLanguage"] as? String ?? "auto" }, set: { send("settings", ["sttLanguage": $0]) })) { Text("自动").tag("auto"); Text("中文").tag("zh"); Text("English").tag("en"); Text("日本語").tag("ja") }
            Text("识别语言决定听哪种语言；英文输出会将听写内容整理后翻译为英文").font(.caption).foregroundStyle(.secondary)
            }
            Section("常用词与快捷短语") { PersonalizationView(model: model) }
            QuietTypeUpdateSettings()
        }.formStyle(.grouped).scrollContentBackground(.hidden).padding(12)
    }
}
private func dateLabel(_ value: String) -> String {
    let parser = ISO8601DateFormatter(); parser.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    let date = parser.date(from: value) ?? ISO8601DateFormatter().date(from: value)
    return date.map { DateFormatter.localizedString(from: $0, dateStyle: .short, timeStyle: .short) } ?? value
}
private struct RecognitionDots: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var started = Date()
    var body: some View {
        TimelineView(.animation(minimumInterval: 1.0 / 60.0, paused: reduceMotion)) { context in
            let elapsed = context.date.timeIntervalSince(started)
            HStack(spacing: 4.5) {
                ForEach(0..<4) { index in
                    let wave = reduceMotion ? 0 : sin(elapsed * 2 * .pi / 1.35 - Double(index) * 0.72)
                    Circle().fill(Color.primary)
                        .frame(width: 4.5, height: 4.5)
                        .scaleEffect(reduceMotion ? 1 : 0.92 + 0.08 * wave)
                        .opacity(reduceMotion ? 0.75 : 0.72 + 0.23 * wave)
                        .offset(y: -3 * wave)
                }
            }.frame(width: 42, height: 18)
        }.accessibilityElement(children: .ignore).accessibilityLabel("正在识别")
            .allowsHitTesting(false)
    }
}

private struct MicrophoneWaveform: View {
    @ObservedObject var meter: MicrophoneState
    let recording: Bool
    let limitWarning: Bool
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var body: some View {
        HStack(spacing: 2) {
            ForEach(0..<11) { index in
                Capsule().frame(width: 2, height: 3 + meter.levels[index] * 17)
            }
        }.accessibilityElement(children: .ignore)
            .accessibilityLabel(!recording ? "正在准备麦克风" : limitWarning ? "麦克风音量，还剩不到 30 秒，即将自动结束" : "麦克风音量")
            .help(!recording ? "正在准备麦克风" : limitWarning ? "还剩不到 30 秒，即将自动结束本段录音" : "录音中，最长 5 分钟")
            .animation(reduceMotion ? nil : .linear(duration: 0.07), value: meter.levels)
    }
}

private struct RecordingView: View {
    @ObservedObject var model: InterfaceState
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private var phase: String { model.hudVoice["phase"] as? String ?? "idle" }
    private var injected: Bool { model.hudVoice["injected"] as? Bool ?? false }
    private var canRetry: Bool { model.hudVoice["canRetry"] as? Bool ?? false }
    private var busy: Bool { ["starting", "stopping", "transcribing", "retrying", "inserting"].contains(phase) }
    private var confirmation: Bool { phase == "success" && injected }
    var body: some View {
        Group {
            if phase == "success" && !injected {
                HStack(spacing: 4) {
                    Text(model.transcript).lineLimit(1).truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .help(model.hudVoice["status"] as? String ?? "文字已保留")
                    if model.hudVoice["canInsert"] as? Bool == true {
                        Button { send("reinsert", ["sessionId": model.sessionId]) } label: {
                            Image(systemName: "arrow.turn.down.right").frame(width: 22, height: 22)
                        }.help("填入当前输入框，无需重新录音").accessibilityLabel("再次填入当前输入框")
                    }
                    CopyButton(text: model.transcript) {
                        let session = model.sessionId
                        // Let the checkmark appear before closing this result,
                        // without cancelling a newer recording.
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.9) {
                            if model.sessionId == session && model.phase == "success" { send("cancel") }
                        }
                    }.id(model.sessionId)
                }
            } else if phase == "success" {
                ZStack {
                    Circle().fill(Color.white)
                    Image(systemName: "checkmark").font(.system(size: 10, weight: .semibold))
                        .foregroundStyle(Color.black)
                }.frame(width: 20, height: 20)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(model.hudVoice["status"] as? String ?? "已填入")
            } else {
                controls
            }
        }.buttonStyle(.plain).font(.system(size: 11, weight: .medium)).padding(.horizontal, 6)
            .frame(width: phase == "success" ? injected ? 54 : 156 : 112, height: 26)
            .background(confirmation ? Color.clear : Color.white)
            .clipShape(RoundedRectangle(cornerRadius: 13, style: .continuous))
            .shadow(color: confirmation ? .clear : .black.opacity(0.10), radius: 3, x: 0, y: 1)
            .preferredColorScheme(.light)
            .scaleEffect(reduceMotion || model.hud.revealed ? 1 : 0.56)
            .opacity(model.hud.revealed ? 1 : 0).allowsHitTesting(model.hud.revealed)
            .frame(width: 172, height: 38)
    }
    private var controls: some View {
        HStack(spacing: 4) {
            Button { send("cancel") } label: {
                Image(systemName: "xmark.circle.fill").symbolRenderingMode(.hierarchical)
                    .font(.system(size: 20, weight: .regular)).foregroundStyle(.secondary)
                    .frame(width: 22, height: 22)
            }
                .help("取消 · Esc").accessibilityLabel("取消录音")
            Group {
                if phase == "starting" || phase == "recording" {
                    MicrophoneWaveform(meter: model.meter, recording: phase == "recording", limitWarning: model.hudVoice["limitWarning"] as? Bool == true)
                } else if busy { RecognitionDots() }
                else if phase == "error" {
                    Image(systemName: "exclamationmark.circle").font(.system(size: 13, weight: .medium))
                        .help(model.hudVoice["error"] as? String ?? "输入失败").accessibilityLabel("输入失败")
                }
            }.frame(maxWidth: .infinity)
            Button { send(phase == "error" ? canRetry ? "retry" : "toggle" : "finish") } label: {
                Image(systemName: phase == "error" ? "arrow.clockwise.circle.fill" : model.hudVoice["limitWarning"] as? Bool == true ? "clock.badge.exclamationmark" : "checkmark.circle.fill")
                    .symbolRenderingMode(.hierarchical).font(.system(size: 20, weight: .regular))
                    .frame(width: 22, height: 22)
            }.disabled(busy).help(phase == "error" ? canRetry ? "重新识别保留录音" : "重新录音" : model.hudVoice["limitWarning"] as? Bool == true ? "即将录满 5 分钟，点击完成" : "完成 · Enter")
                .accessibilityLabel(phase == "error" ? canRetry ? "重新识别保留录音" : "重新录音" : "完成录音")
        }
    }
}

@_cdecl("quiet_ui_init")
public func quietUIInit(_ handler: @escaping @convention(c) (UnsafePointer<CChar>) -> Void) {
    callback = handler
    let work = {
        QuietTypeUpdater.shared.isBusy = {
            state.active || !["ready", "error"].contains(state.runtimePhase)
        }
        QuietTypeUpdater.shared.start()
        installFnEventTap()
        fnLocalMonitor = NSEvent.addLocalMonitorForEvents(matching: [.flagsChanged, .keyDown]) { event in
            fnEventTap == nil && handleFn(event) ? nil : event
        }
        fnGlobalMonitor = NSEvent.addGlobalMonitorForEvents(matching: [.flagsChanged, .keyDown]) { event in
            if fnEventTap == nil { _ = handleFn(event) }
        }
        // Retry after an Accessibility grant; never install both delivery paths.
        fnTapTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in installFnEventTap() }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 660, height: 540), styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = "QuietType"; window.isReleasedWhenClosed = false; window.backgroundColor = .white
        window.minSize = NSSize(width: 520, height: 460); window.contentView = HostingView(rootView: MainView(model: state)); window.delegate = delegate
        window.center(); mainWindow = window
        let panel = RecordingPanel(contentRect: NSRect(x: 0, y: 0, width: 172, height: 38), styleMask: [.nonactivatingPanel, .borderless], backing: .buffered, defer: false)
        panel.title = "QuietType 录音"; panel.isOpaque = false; panel.backgroundColor = .clear
        panel.hasShadow = false
        panel.animationBehavior = .none
        panel.titleVisibility = .hidden; panel.titlebarAppearsTransparent = true
        [.closeButton, .miniaturizeButton, .zoomButton].forEach { panel.standardWindowButton($0)?.isHidden = true }
        panel.isFloatingPanel = true; panel.hidesOnDeactivate = false; panel.becomesKeyOnlyIfNeeded = true
        panel.level = .floating; panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]; panel.isMovableByWindowBackground = true
        panel.contentView = HostingView(rootView: RecordingView(model: state)); recordingPanel = panel
        window.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true)
    }
    if Thread.isMainThread { work() } else { DispatchQueue.main.async(execute: work) }
}
@_cdecl("quiet_ui_show")
public func quietUIShow() { DispatchQueue.main.async { mainWindow?.makeKeyAndOrderFront(nil); NSApp.activate(ignoringOtherApps: true); send("refresh") } }
@_cdecl("quiet_ui_hide_hud")
public func quietUIHideHUD() { DispatchQueue.main.async { if state.phase == "idle" { hideHUD() } } }
struct InterfaceUpdate {
    private let packet: [String: Any]
    init?(_ value: String) {
        guard let data = value.data(using: .utf8), let packet = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return nil }
        self.packet = packet
    }
    func apply(to state: InterfaceState) {
        if let settings = packet["settings"] as? [String: Any] { state.settings = settings }
        if let granted = packet["accessibilityGranted"] as? Bool { state.accessibilityGranted = granted }
        if let status = packet["microphoneStatus"] as? Int { state.microphoneStatus = status }
        if let sent = packet["firstInputSent"] as? Bool { state.firstInputSent = sent }
        if let runtime = packet["runtime"] as? [String: Any] {
            state.runtimePhase = runtime["phase"] as? String ?? "checking"
            state.runtimeMessage = runtime["message"] as? String ?? "正在准备本地语音"
            state.runtimeReady = runtime["ready"] as? Bool ?? false
        }
        if let personal = packet["personal"] as? [String: Any] {
            state.words = personal["words"] as? [[String: Any]] ?? []
            state.snippets = personal["snippets"] as? [[String: Any]] ?? []
        }
        if let busy = packet["personalBusy"] as? Bool { state.personalBusy = busy }
        if let saved = packet["personalSaved"] as? String { state.didSavePersonal(saved) }
        if let voice = packet["voice"] as? [String: Any],
           let sessionId = (voice["sessionId"] as? NSNumber)?.uint64Value, sessionId >= state.sessionId {
            let previous = state.phase
            let phase = voice["phase"] as? String ?? "idle"
            let elapsed = voice["elapsedSeconds"] as? Int ?? 0
            let warning = voice["limitWarning"] as? Bool ?? false
            if sessionId == state.sessionId, previous == "recording", phase == "recording" {
                // A recording tick changes the clock, not the rest of the interface.
                if state.elapsedSeconds != elapsed { state.elapsedSeconds = elapsed }
                if state.limitWarning != warning {
                    state.limitWarning = warning
                    state.hudVoice["limitWarning"] = warning
                }
            } else {
                let newSession = sessionId != state.sessionId
                state.phase = phase; state.status = voice["status"] as? String ?? ""
                // Keep the last visible snapshot during the exit animation.
                if phase != "idle" { state.hudVoice = voice }
                state.sessionId = sessionId
                state.error = voice["error"] as? String ?? ""; state.canRetry = voice["canRetry"] as? Bool ?? false
                state.injected = voice["injected"] as? Bool ?? false
                state.canInsert = voice["canInsert"] as? Bool ?? false
                state.elapsedSeconds = elapsed; state.limitWarning = warning
                if let text = voice["transcript"] as? String, !text.isEmpty { state.transcript = text }
                if previous != phase && state.settings["notifySound"] as? Bool == true {
                    if phase == "recording" { playStartSound(state.settings["startSound"] as? String ?? "soft") }
                    else if phase == "error" { NSSound(named: "Basso")?.play() }
                }
                if ["starting", "recording"].contains(phase), previous != phase || newSession { state.meter.reset() }
                if phase == "idle" { hideHUD(for: state) }
                else { showHUD(for: state) }
            }
        }
        if let levels = packet["audioLevels"] as? [Double], state.phase == "recording",
           (packet["sessionId"] as? NSNumber)?.uint64Value == state.sessionId { state.meter.update(levels) }
        if let ready = packet["shortcutReady"] as? Bool { state.shortcutReady = ready }
        if let history = packet["history"] as? [[String: Any]] {
            state.records = packet["append"] as? Bool == true ? state.records + history : history
            state.more = history.count == 50
            if state.transcript.isEmpty, let row = state.records.first { state.transcript = row["refinedText"] as? String ?? row["editedText"] as? String ?? "" }
        }
        if packet["historyChanged"] as? Bool == true { send("history", ["query": state.historyQuery, "offset": 0]) }
        if let busy = packet["historyBusy"] as? Bool { state.historyBusy = busy }
        if let notice = packet["notice"] as? String {
            state.notice = notice
            DispatchQueue.main.asyncAfter(deadline: .now() + 6) { if state.notice == notice { state.notice = "" } }
        }
    }
}
@_cdecl("quiet_ui_update")
public func quietUIUpdate(_ pointer: UnsafePointer<CChar>) {
    guard let update = InterfaceUpdate(String(cString: pointer)) else { return }
    DispatchQueue.main.async { update.apply(to: state) }
}
