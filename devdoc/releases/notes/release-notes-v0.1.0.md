# Lexift v0.1.0

Lexift V0.1 是首个 Windows 桌面版本，打通了从全局快捷键、选中文字、DeepL 翻译到被动 Popup 展示的完整链路。

## 功能

- `Alt + X` 全局划词翻译，可在 Settings 中即时修改快捷键。
- UI Automation 优先、Clipboard fallback 的 Windows 选区获取。
- DeepL 翻译与系统 Credential Manager 安全凭证存储。
- Slint 主窗口、翻译 Popup、Settings 与系统托盘后台生命周期。
- 目标语言、Provider、快捷键与开机启动的单项自动保存。
- 单实例运行；再次启动会打开已有实例。
- 当前用户 NSIS 安装器和 portable ZIP。

## 数据与隐私

- API Key 不写入普通配置文件或日志。
- Clipboard fallback 会恢复原有文本、图片或文件剪贴板内容。
- 本地滚动日志最多保留 5 个文件，且不记录选中文字和翻译源文本。
- 卸载时可选择保留设置、日志和 API Key，或勾选“删除用户数据”将它们一并清除。

## 已知限制

- V0.1 仅正式支持 Windows x86_64 和 DeepL。
- 高权限应用中的选区读取受 Windows 完整性级别限制，Lexift 不自动提权。
- 暂不包含 OCR、图片翻译、TTS、自动更新、macOS 或 Linux 发行包。
