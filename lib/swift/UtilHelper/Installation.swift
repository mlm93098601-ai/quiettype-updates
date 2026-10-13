import AppKit

func replaceInstallation(with staged: URL, at destination: URL) throws {
    let files = FileManager.default
    if files.fileExists(atPath: destination.path) {
        _ = try files.replaceItemAt(destination, withItemAt: staged, options: .usingNewMetadataOnly)
    } else {
        try files.moveItem(at: staged, to: destination)
    }
}

@_cdecl("quiet_prepare_installation")
public func prepareQuietInstallation() -> Bool {
    let source = Bundle.main.bundleURL
    let destination = URL(fileURLWithPath: "/Applications/QuietType.app")
    guard source.pathExtension == "app", source.standardizedFileURL.path != destination.standardizedFileURL.path,
          Bundle.main.bundleIdentifier == "com.ai-quick-input.assistant" else { return false }
    let work = {
        do {
            let other = NSRunningApplication.runningApplications(withBundleIdentifier: "com.ai-quick-input.assistant")
                .contains { $0.processIdentifier != ProcessInfo.processInfo.processIdentifier }
            guard !other else {
                throw NSError(domain: "QuietType.Install", code: 1, userInfo: [NSLocalizedDescriptionKey:
                    "请先退出正在运行的 QuietType，再打开这份下载的应用；已有用户也可在设置中直接检查更新"])
            }
            let files = FileManager.default
            if files.fileExists(atPath: destination.path) {
                let values = try destination.resourceValues(forKeys: [.isSymbolicLinkKey])
                guard values.isSymbolicLink != true,
                      Bundle(url: destination)?.bundleIdentifier == "com.ai-quick-input.assistant" else {
                    throw NSError(domain: "QuietType.Install", code: 2, userInfo: [NSLocalizedDescriptionKey:
                        "应用程序中存在其他同名项目，已保留原文件，请先检查 QuietType.app"])
                }
            }
            let temporary = try files.url(for: .itemReplacementDirectory, in: .userDomainMask,
                                          appropriateFor: destination, create: true)
            defer { try? files.removeItem(at: temporary) }
            let staged = temporary.appendingPathComponent("QuietType.app")
            try files.copyItem(at: source, to: staged)
            let check = Process()
            check.executableURL = URL(fileURLWithPath: "/usr/bin/codesign")
            check.arguments = ["--verify", "--deep", "--strict", staged.path]
            check.standardOutput = FileHandle.nullDevice; check.standardError = FileHandle.nullDevice
            try check.run(); check.waitUntilExit()
            guard check.terminationStatus == 0 else {
                throw NSError(domain: "QuietType.Install", code: 3, userInfo: [NSLocalizedDescriptionKey:
                    "下载的应用校验失败，请重新下载最新版 QuietType"])
            }
            try replaceInstallation(with: staged, at: destination)
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.createsNewApplicationInstance = true
            configuration.activates = true
            NSWorkspace.shared.openApplication(at: destination, configuration: configuration) { _, error in
                DispatchQueue.main.async {
                    if let error {
                        let alert = NSAlert(); alert.messageText = "QuietType 已安装"
                        alert.informativeText = "请从应用程序中打开 QuietType；如果 macOS 阻止打开，请到系统设置的隐私与安全中确认\n\(error.localizedDescription)"
                        alert.runModal()
                    }
                    NSApp.terminate(nil)
                }
            }
        } catch {
            let alert = NSAlert(); alert.messageText = "暂时无法自动安装 QuietType"
            alert.informativeText = "\(error.localizedDescription)\n你也可以把 QuietType 拖入应用程序后打开，已有设置和记录会保留"
            alert.runModal()
            DispatchQueue.main.async { NSApp.terminate(nil) }
        }
    }
    if Thread.isMainThread { work() } else { DispatchQueue.main.sync(execute: work) }
    return true
}
