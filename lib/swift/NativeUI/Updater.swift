import AppKit
import SwiftUI
import Sparkle

// Sparkle owns download validation, replacement, relaunch and update scheduling.
final class QuietTypeUpdater: NSObject, ObservableObject, SPUUpdaterDelegate {
    static let shared = QuietTypeUpdater()
    @Published var canCheck = false
    @Published var automatic = false
    @Published var configured = false
    private var controller: SPUStandardUpdaterController?
    private var observations: [NSKeyValueObservation] = []
    var isBusy: () -> Bool = { false }

    func start() {
        guard controller == nil,
              let value = Bundle.main.object(forInfoDictionaryKey: "SUFeedURL") as? String,
              let url = URL(string: value), url.scheme == "https", url.host != nil,
              let key = Bundle.main.object(forInfoDictionaryKey: "SUPublicEDKey") as? String,
              Data(base64Encoded: key)?.count == 32 else { return }
        let controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: self, userDriverDelegate: nil)
        self.controller = controller
        configured = true
        observations = [
            controller.updater.observe(\.canCheckForUpdates, options: [.initial, .new]) { [weak self] updater, _ in
                DispatchQueue.main.async { self?.canCheck = updater.canCheckForUpdates }
            },
            controller.updater.observe(\.automaticallyChecksForUpdates, options: [.initial, .new]) { [weak self] updater, _ in
                DispatchQueue.main.async { self?.automatic = updater.automaticallyChecksForUpdates }
            }
        ]
        controller.startUpdater()
    }
    func check() { controller?.checkForUpdates(nil) }
    func setAutomatic(_ value: Bool) { controller?.updater.automaticallyChecksForUpdates = value }
    func updater(_ updater: SPUUpdater, mayPerform updateCheck: SPUUpdateCheck) throws {
        if isBusy() {
            throw NSError(domain: "QuietType.Updater", code: 1, userInfo: [NSLocalizedDescriptionKey: "请等待语音准备完成，或结束当前录音和识别，再检查更新"])
        }
    }
    func updater(_ updater: SPUUpdater, shouldProceedWithUpdate updateItem: SUAppcastItem, updateCheck: SPUUpdateCheck) throws {
        if isBusy() {
            throw NSError(domain: "QuietType.Updater", code: 2, userInfo: [NSLocalizedDescriptionKey: "当前正在准备语音、录音或识别，请完成后再更新"])
        }
    }
    func updater(_ updater: SPUUpdater, shouldPostponeRelaunchForUpdate item: SUAppcastItem, untilInvokingBlock installHandler: @escaping () -> Void) -> Bool {
        guard isBusy() else { return false }
        waitUntilIdle(installHandler)
        return true
    }
    private func waitUntilIdle(_ handler: @escaping () -> Void) {
        DispatchQueue.main.asyncAfter(deadline: .now() + 1) {
            if self.isBusy() { self.waitUntilIdle(handler) } else { handler() }
        }
    }
}

struct QuietTypeUpdateSettings: View {
    @ObservedObject private var updater = QuietTypeUpdater.shared
    var body: some View {
        Section("软件更新") {
            HStack {
                Text("版本 \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "—")")
                Spacer()
                Button("检查更新…") { updater.check() }.disabled(!updater.canCheck)
            }
            Toggle("每天自动检查更新", isOn: Binding(get: { updater.automatic }, set: { updater.setAutomatic($0) }))
                .disabled(!updater.configured)
            Text(updater.configured ? "发现新版本时提示，确认后下载并重新打开软件" : "更新地址尚未配置，此包可用于安装验证")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}
