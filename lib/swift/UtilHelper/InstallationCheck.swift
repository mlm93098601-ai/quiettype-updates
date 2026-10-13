import AppKit

@main struct InstallationCheck {
    static func main() throws {
        let files = FileManager.default
        let root = files.temporaryDirectory.appendingPathComponent("quiettype-install-check-\(UUID().uuidString)")
        try files.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? files.removeItem(at: root) }
        let destination = root.appendingPathComponent("installed")
        let staged = root.appendingPathComponent("candidate")
        try files.createDirectory(at: staged, withIntermediateDirectories: true)
        try Data("first".utf8).write(to: staged.appendingPathComponent("content"))
        try replaceInstallation(with: staged, at: destination)
        let first = try String(contentsOf: destination.appendingPathComponent("content"), encoding: .utf8)
        assert(first == "first")
        try files.createDirectory(at: staged, withIntermediateDirectories: true)
        try Data("new".utf8).write(to: staged.appendingPathComponent("content"))
        try replaceInstallation(with: staged, at: destination)
        assert((try? String(contentsOf: destination.appendingPathComponent("content"), encoding: .utf8)) == "new")
        do { try replaceInstallation(with: root.appendingPathComponent("missing"), at: destination); assertionFailure("Missing source accepted") }
        catch { assert((try? String(contentsOf: destination.appendingPathComponent("content"), encoding: .utf8)) == "new") }
        assert(!files.fileExists(atPath: staged.path))
        print("First install, atomic replacement, failed replacement preservation and staging cleanup passed")
    }
}
