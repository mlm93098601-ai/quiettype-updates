import Foundation
import ApplicationServices
import AppKit
import os

private let inputLog = Logger(subsystem: "com.ai-quick-input.assistant", category: "InputHelper")

private func onMain<T>(_ action: () -> T) -> T {
    if Thread.isMainThread { return action() }
    return DispatchQueue.main.sync(execute: action)
}

// Only the session token is retained while recording, never an input field.
struct InputSession {
    private var latest: UInt64 = 0
    private var active: UInt64?
    mutating func begin(_ session: UInt64) -> Bool {
        guard session > latest else { return false }
        latest = session; active = session; return true
    }
    mutating func clear() { active = nil }
    func accepts(_ session: UInt64) -> Bool { active == session }
}
private var inputSession = InputSession()
private var activationObserver: NSObjectProtocol?
private var preparedApplications = Set<pid_t>()
private let ownPID = ProcessInfo.processInfo.processIdentifier
private var focusDeadline = Double.infinity
private var refusal: Int32 = -4

private func reject(_ code: Int32) -> Int32 {
    refusal = code
    return code
}

private func attribute(_ element: AXUIElement, _ name: String) -> AnyObject? {
    guard ProcessInfo.processInfo.systemUptime < focusDeadline else {
        refusal = -11; return nil
    }
    AXUIElementSetMessagingTimeout(element, 0.08)
    var value: AnyObject?
    let error = AXUIElementCopyAttributeValue(element, name as CFString, &value)
    return error == .success ? value : nil
}
private func axElement(_ value: AnyObject?) -> AXUIElement? {
    guard let value = value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
    return (value as! AXUIElement)
}
private func prepareAccessibility(_ app: NSRunningApplication, force: Bool = false) {
    guard app.processIdentifier != ownPID, AXIsProcessTrusted(),
          force || !preparedApplications.contains(app.processIdentifier) else { return }
    let element = AXUIElementCreateApplication(app.processIdentifier)
    AXUIElementSetMessagingTimeout(element, 0.08)
    // Browsers build their AX tree lazily. Prepare it on activation and again
    // at writeback; unsupported native apps simply ignore this.
    _ = attribute(element, kAXRoleAttribute)
    let manual = AXUIElementSetAttributeValue(element, "AXManualAccessibility" as CFString, kCFBooleanTrue)
    let enhanced = manual == .success ? manual : AXUIElementSetAttributeValue(element, "AXEnhancedUserInterface" as CFString, kCFBooleanTrue)
    // Failed/unsupported requests must not poison preparation for the lifetime
    // of a process. Electron can also disable its tree after it was enabled.
    if manual == .success || enhanced == .success { preparedApplications.insert(app.processIdentifier) }
}
@_cdecl("init_input_tracking")
public func initInputTracking() {
    onMain {
        if let app = NSWorkspace.shared.frontmostApplication { prepareAccessibility(app) }
        activationObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didActivateApplicationNotification, object: nil, queue: .main
        ) { notification in
            if let app = notification.userInfo?[NSWorkspace.applicationUserInfoKey] as? NSRunningApplication {
                prepareAccessibility(app, force: true)
            }
        }
    }
}
@_cdecl("begin_input_session")
public func beginInputSession(_ session: UInt64) -> Bool { onMain { inputSession.begin(session) } }
@_cdecl("clear_input_target")
public func clearInputTarget() { onMain { inputSession.clear() } }

func editableRole(_ role: String, selection: Bool, writable: Bool, explicitEditable: Bool? = nil) -> Bool {
    guard explicitEditable != false,
          !["AXButton", "AXLink", "AXSlider", "AXCheckBox", "AXSecureTextField"].contains(role) else { return false }
    return ["AXTextField", "AXTextArea", "AXComboBox"].contains(role) || explicitEditable == true || (selection && writable)
}
func focusedElementBelongsToApp(elementPID: pid_t, appPID: pid_t, reportedByApp: Bool, sameWindow: Bool) -> Bool {
    elementPID == appPID || reportedByApp || sameWindow
}
func allowsDisabledDocument(bundle: String?, role: String, selection: Bool) -> Bool {
    // Word reports AXEnabled=false even for its active, editable document body.
    // Its native Paste command remains the final editability gate.
    bundle == "com.microsoft.Word" && role == "AXTextArea" && selection
}
private func isEditable(_ element: AXUIElement) -> Bool {
    let role = attribute(element, kAXRoleAttribute) as? String ?? ""
    let subrole = attribute(element, kAXSubroleAttribute) as? String ?? ""
    if role == "AXSecureTextField" || subrole == "AXSecureTextField" { refusal = -7; return false }
    if attribute(element, kAXEnabledAttribute) as? Bool == false {
        var pid: pid_t = 0
        guard AXUIElementGetPid(element, &pid) == .success,
              allowsDisabledDocument(bundle: NSRunningApplication(processIdentifier: pid)?.bundleIdentifier,
                                     role: role, selection: attribute(element, kAXSelectedTextRangeAttribute) != nil) else { return false }
    }
    let explicit = attribute(element, "AXEditable") as? Bool
    if editableRole(role, selection: false, writable: false, explicitEditable: explicit) { return true }
    guard attribute(element, kAXSelectedTextRangeAttribute) != nil else { return false }
    var settable = DarwinBoolean(false)
    return editableRole(role, selection: true,
                        writable: AXUIElementIsAttributeSettable(element, kAXSelectedTextAttribute as CFString, &settable) == .success && settable.boolValue,
                        explicitEditable: explicit)
}
private func editableAncestor(_ focused: AXUIElement) -> AXUIElement? {
    var current: AXUIElement? = focused
    var visited: [AXUIElement] = []
    for _ in 0..<32 {
        guard let element = current, ProcessInfo.processInfo.systemUptime < focusDeadline,
              !visited.contains(where: { CFEqual($0, element) }) else { break }
        visited.append(element)
        if isEditable(element) { return element }
        if refusal == -7 { return nil }
        current = axElement(attribute(element, kAXParentAttribute))
    }
    return nil
}
private func focusedDescendant(_ root: AXUIElement) -> AXUIElement? {
    // Inspect only keyboard-focused candidates, never the first visible editor.
    // A web area can own focus while its active editor is several levels below.
    var pending: [(AXUIElement, Int)] = [(root, 0)]
    var visited: [AXUIElement] = []
    while let (element, depth) = pending.popLast(), visited.count < 256,
          ProcessInfo.processInfo.systemUptime < focusDeadline {
        if visited.contains(where: { CFEqual($0, element) }) { continue }
        visited.append(element)
        if attribute(element, kAXFocusedAttribute) as? Bool == true && isEditable(element) { return element }
        if refusal == -7 { return nil }
        guard depth < 32 else { continue }
        let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
        pending.append(contentsOf: children.prefix(256).reversed().map { ($0, depth + 1) })
    }
    return nil
}
private struct CurrentInput {
    let app: NSRunningApplication
    let element: AXUIElement
    let reportedFocus: AXUIElement
    let applicationFocus: Bool
    let focusAttribute: String
    let requireEditorFocus: Bool
}
private func currentInput() -> CurrentInput? {
    refusal = -4
    guard let app = NSWorkspace.shared.frontmostApplication, app.processIdentifier != ownPID, !app.isTerminated else { refusal = -3; return nil }
    focusDeadline = ProcessInfo.processInfo.systemUptime + 0.6
    defer { focusDeadline = .infinity }
    // Re-enable at writeback, not only at process activation. This also repairs
    // apps whose AX tree was disabled/recreated without a process restart.
    prepareAccessibility(app, force: true)
    let application = AXUIElementCreateApplication(app.processIdentifier)
    let system = AXUIElementCreateSystemWide()
    let direct = axElement(attribute(application, kAXFocusedUIElementAttribute))
    let fallback = axElement(attribute(system, kAXFocusedUIElementAttribute))
    let window = axElement(attribute(application, kAXFocusedWindowAttribute))
    var roots: [AXUIElement] = []
    for (focused, byApp, focusAttribute) in [(direct, true, kAXFocusedUIElementAttribute),
                                            (fallback, false, kAXFocusedUIElementAttribute),
                                            (window, true, kAXFocusedWindowAttribute)] {
        guard let focused else { continue }
        if roots.contains(where: { CFEqual($0, focused) }) { continue }
        roots.append(focused)
        var pid: pid_t = 0
        guard AXUIElementGetPid(focused, &pid) == .success, pid != ownPID else { continue }
        var sameWindow = false
        if pid != app.processIdentifier && !byApp,
           let window = axElement(attribute(focused, kAXWindowAttribute)),
           let frontWindow = axElement(attribute(application, kAXFocusedWindowAttribute)) {
            sameWindow = CFEqual(window, frontWindow)
        }
        guard focusedElementBelongsToApp(elementPID: pid, appPID: app.processIdentifier, reportedByApp: byApp, sameWindow: sameWindow) else { refusal = -5; continue }
        let ancestor = editableAncestor(focused)
        let element = ancestor ?? (refusal == -7 ? nil : focusedDescendant(focused))
        if let element {
            return CurrentInput(app: app, element: element, reportedFocus: focused, applicationFocus: byApp,
                                focusAttribute: focusAttribute, requireEditorFocus: ancestor == nil)
        }
        if refusal == -7 { return nil }
        if refusal != -11 { refusal = -6 }
    }
    return nil
}
private func stillFocused(_ target: CurrentInput) -> Bool {
    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == target.app.processIdentifier,
          !target.app.isTerminated else { return false }
    if target.requireEditorFocus && attribute(target.element, kAXFocusedAttribute) as? Bool != true { return false }
    let root = target.applicationFocus ? AXUIElementCreateApplication(target.app.processIdentifier) : AXUIElementCreateSystemWide()
    guard let now = axElement(attribute(root, target.focusAttribute)) else { return false }
    return CFEqual(now, target.reportedFocus) || CFEqual(now, target.element)
}
func requiresSystemPaste(bundle: String?) -> Bool {
    // These editors need normal system paste events to update application state.
    // AXSelectedText can acknowledge a write without updating their composer.
    ["com.microsoft.Word", "com.tencent.xinWeChat", "com.todesktop.230313mzl4w4u92", "com.openai.codex", "com.google.Chrome", "com.apple.Safari",
     "com.microsoft.edgemac", "com.brave.Browser", "company.thebrowser.Browser",
     "org.mozilla.firefox"].contains(bundle ?? "")
}
private func webEditor(_ element: AXUIElement, bundle: String?) -> Bool {
    if requiresSystemPaste(bundle: bundle) { return true }
    var current: AXUIElement? = element
    for _ in 0..<32 {
        guard let node = current else { break }
        if attribute(node, kAXRoleAttribute) as? String == "AXWebArea" { return true }
        current = axElement(attribute(node, kAXParentAttribute))
    }
    return false
}

private class PasteboardState {
    private var items: [[NSPasteboard.PasteboardType: Data]] = []
    func save() {
        items = (NSPasteboard.general.pasteboardItems ?? []).map { item in
            var data: [NSPasteboard.PasteboardType: Data] = [:]
            for type in item.types { if let value = item.data(forType: type) { data[type] = value } }
            return data
        }
    }
    func restore() {
        let restored = items.map { data -> NSPasteboardItem in
            let item = NSPasteboardItem()
            for (type, value) in data { item.setData(value, forType: type) }
            return item
        }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.writeObjects(restored)
    }
}

private func withTemporaryClipboard(_ string: String, paste: () -> Int32) -> Int32 {
    let saved = PasteboardState()
    saved.save()
    let pasteboard = NSPasteboard.general
    pasteboard.clearContents()
    let item = NSPasteboardItem()
    item.setString(string, forType: .string)
    item.setData(Data(), forType: NSPasteboard.PasteboardType("org.nspasteboard.TransientType"))
    guard pasteboard.writeObjects([item]) else { saved.restore(); return reject(-10) }
    let written = pasteboard.changeCount
    let result = paste()
    if result > 0 {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
            // Never overwrite a newer copy made by the user.
            if pasteboard.changeCount == written { saved.restore() }
        }
    } else if pasteboard.changeCount == written {
        saved.restore()
    }
    return result
}

func allowsWeChatMenuPaste(bundle: String?, refusal: Int32) -> Bool {
    bundle == "com.tencent.xinWeChat" && [-4, -6].contains(refusal)
}
func isNativePasteMenu(role: String?, identifier: String?, command: String?, modifiers: Int?) -> Bool {
    role == "AXMenuItem" && (identifier == "paste:" || (command?.lowercased() == "v" && modifiers == 0))
}
private func nativePasteMenu(_ application: AXUIElement) -> AXUIElement? {
    guard let menu = axElement(attribute(application, kAXMenuBarAttribute)) else { return nil }
    var pending: [(AXUIElement, Int)] = [(menu, 0)]
    var visited: [AXUIElement] = []
    while let (element, depth) = pending.popLast(), visited.count < 256,
          ProcessInfo.processInfo.systemUptime < focusDeadline {
        if visited.contains(where: { CFEqual($0, element) }) { continue }
        visited.append(element)
        let role = attribute(element, kAXRoleAttribute) as? String
        if role == "AXMenuItem" {
            let identifier = attribute(element, kAXIdentifierAttribute) as? String
            if identifier == "paste:" || isNativePasteMenu(role: role, identifier: identifier,
                                   command: attribute(element, kAXMenuItemCmdCharAttribute) as? String,
                                   modifiers: attribute(element, kAXMenuItemCmdModifiersAttribute) as? Int) {
                return element
            }
        }
        guard depth < 4 else { continue }
        let children = attribute(element, kAXChildrenAttribute) as? [AXUIElement] ?? []
        pending.append(contentsOf: children.prefix(256).reversed().map { ($0, depth + 1) })
    }
    return nil
}
private func weChatMenuPaste(_ string: String, refusal originalRefusal: Int32) -> Int32 {
    guard let app = NSWorkspace.shared.frontmostApplication,
          allowsWeChatMenuPaste(bundle: app.bundleIdentifier, refusal: originalRefusal),
          !app.isTerminated else { return reject(originalRefusal) }
    focusDeadline = ProcessInfo.processInfo.systemUptime + 0.6
    defer { focusDeadline = .infinity }
    let application = AXUIElementCreateApplication(app.processIdentifier)
    guard let window = axElement(attribute(application, kAXFocusedWindowAttribute)) else { return reject(refusal) }
    // Some WeChat versions expose windows/menus but omit the chat editor.
    // Its native Paste command is enabled only when the current view accepts it.
    return withTemporaryClipboard(string) {
        // Paste availability can depend on clipboard type, so resolve it only
        // after the temporary text has been installed.
        guard let menu = nativePasteMenu(application) else { return reject(refusal) }
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == app.processIdentifier,
              !app.isTerminated,
              let now = axElement(attribute(application, kAXFocusedWindowAttribute)),
              CFEqual(now, window) else { return reject(-8) }
        guard attribute(menu, kAXEnabledAttribute) as? Bool == true else { return reject(refusal) }
        guard ProcessInfo.processInfo.systemUptime < focusDeadline else { return reject(-11) }
        guard AXUIElementPerformAction(menu, kAXPressAction as CFString) == .success else { return reject(-9) }
        inputLog.info("WeChat native paste dispatched")
        return 1
    }
}

// 2 = AX replacement, 1 = dispatched paste; negative values explain refusal.
@_cdecl("insert_text")
public func insertText(_ text: UnsafePointer<CChar>, _ session: UInt64) -> Int32 {
    let string = String(cString: text)
    return onMain {
        guard AXIsProcessTrusted() else { return reject(-1) }
        guard inputSession.accepts(session) else { return reject(-2) }
        guard let target = currentInput() else {
            let result = weChatMenuPaste(string, refusal: refusal)
            inputLog.info("No editable target; insertion result: \(result)")
            return result
        }
        // Read the current field only now, after recognition. Browser editors need
        // normal paste/input events to update both DOM and application state.
        let systemPaste = webEditor(target.element, bundle: target.app.bundleIdentifier)
        guard stillFocused(target) else { return reject(-8) }
        var settable = DarwinBoolean(false)
        if !systemPaste,
           AXUIElementIsAttributeSettable(target.element, kAXSelectedTextAttribute as CFString, &settable) == .success,
           settable.boolValue,
           AXUIElementSetAttributeValue(target.element, kAXSelectedTextAttribute as CFString, string as CFString) == .success {
            return 2
        }
        // Browser/Electron editors may ignore synthetic Command-V. Prefer the
        // app's native Paste action, but only for the already validated editor.
        if systemPaste {
            focusDeadline = ProcessInfo.processInfo.systemUptime + 0.6
            defer { focusDeadline = .infinity }
            let application = AXUIElementCreateApplication(target.app.processIdentifier)
            let result = withTemporaryClipboard(string) {
                guard let menu = nativePasteMenu(application) else { return 0 }
                guard attribute(menu, kAXEnabledAttribute) as? Bool == true else {
                    return target.app.bundleIdentifier == "com.microsoft.Word" ? reject(-6) : 0
                }
                guard stillFocused(target) else { return reject(-8) }
                guard AXUIElementPerformAction(menu, kAXPressAction as CFString) == .success else { return 0 }
                inputLog.info("Native paste dispatched for \(target.app.bundleIdentifier ?? "unknown", privacy: .public)")
                return 1
            }
            if result != 0 { return result }
            if refusal == -11 { return reject(-11) }
            // Word's disabled AX document cannot independently prove it is
            // writable. Never fall back to a keystroke without native approval.
            if target.app.bundleIdentifier == "com.microsoft.Word" { return reject(-6) }
        }
        guard let source = CGEventSource(stateID: .privateState),
              let down = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: 9, keyDown: false) else {
            return reject(-9)
        }
        return withTemporaryClipboard(string) {
            guard stillFocused(target) else { return reject(-8) }
            down.flags = .maskCommand
            up.flags = .maskCommand
            if systemPaste {
                down.post(tap: .cghidEventTap)
                up.post(tap: .cghidEventTap)
            } else {
                down.postToPid(target.app.processIdentifier)
                up.postToPid(target.app.processIdentifier)
            }
            inputLog.info("Keyboard paste dispatched for \(target.app.bundleIdentifier ?? "unknown", privacy: .public)")
            return 1
        }
    }
}
