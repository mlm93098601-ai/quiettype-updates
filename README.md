# QuietType 下载与更新

当前版本：[QuietType 0.2.5](https://github.com/mlm93098601-ai/quiettype-updates/releases/tag/v0.2.5)。修复微信与 Codex 的自动填入兼容问题，优先使用应用原生粘贴，并保留焦点与剪贴板保护。已安装的朋友直接在设置中检查更新；首次安装仍使用下方完整安装包，再检查更新。

Apple 芯片（M 系列）· macOS 26 或更高版本。语音识别和文字整理在本机运行。

## 第一次安装

[下载 QuietType 0.2.0 下载器 ZIP](https://github.com/mlm93098601-ai/quiettype-updates/releases/download/v0.2.0/QuietType-0.2.0-Downloader.zip)

解压 ZIP，双击里面的 `双击下载完整安装包.command`。它使用 macOS 自带工具下载约 6.8GB 的完整离线安装包，校验、合并后自动打开 DMG。下载中断后重新双击即可继续。建议预留至少 30GB 空间。

打开 DMG 后，将 QuietType 拖入 Applications，然后从应用程序中打开。首次受阻时，进入系统设置 → 隐私与安全 → 仍要打开，再授权麦克风和辅助功能。首次启动需要复制离线模型，请等待。

[完整安装包版本页面和校验信息](https://github.com/mlm93098601-ai/quiettype-updates/releases/tag/v0.2.0)

由于 GitHub 单附件限制，DMG 拆成多个分段；下载器会自动完成下载和合并，无需手动处理分段。

## 后续更新

在 QuietType 的设置 → 软件更新中检查新版本，默认每天自动检查。普通程序更新约 7.1MB，复用已有离线模型。

此应用用于朋友之间的小范围分发，使用固定本地签名，尚未经过 Apple 公证。更新由 Sparkle 检查，更新包和更新源均进行 Ed25519 签名校验。

完整安装包包括离线模型和运行环境；普通更新只包含程序。此仓库不包含用户语音、历史记录或签名私钥。联网用于下载和软件更新，识别与整理仍在本机完成。

源码：[下载最新源码（0.2.5，非编译版）](https://github.com/mlm93098601-ai/quiettype-updates/releases/latest/download/QuietType-Source.zip)
