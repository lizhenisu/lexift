# Lexift v0.1.0 Release Candidate Validation

日期：2026-09-20
平台：Windows x86_64
签名状态：Unsigned（未配置 Windows 代码签名证书）

## 自动门禁

- `cargo fmt --all -- --check`
- 默认与 `m1-demo` Workspace Clippy（warnings denied）
- 默认与 `m1-demo` Workspace tests
- Windows Credential Manager、autostart、clipboard、hotkey、single-instance、tray 和 window-context ignored integration tests
- DeepL 真实 API 成功与无效凭证测试
- release build、NSIS packaging 与 `scripts/verify-release.ps1`
- Setup/portable SHA256 与 portable 内容审计

以上检查均通过。Portable 只包含 `lexift.exe`、`LICENSE` 和 `README.md`；没有 `.env`、配置、证书或个人数据。EXE 与 Setup 均明确为 unsigned。

## 本机安装与生命周期验证

- 当前用户静默安装成功，安装路径为 `%LOCALAPPDATA%\Lexift`，无需管理员权限。
- Add/Remove Programs 信息、开始菜单快捷方式、应用文件、卸载器和 PE 元数据正确。
- `InstallLocation` 使用无引号目录值；RC 验证期间发现并修复了旧模板写入带引号值的问题。
- `--background` 只创建一个隐藏主窗口的进程；再次普通启动仍只有一个进程，并显示已有主窗口。
- 启动项可从旧路径自动修复为当前安装路径；配置关闭开机启动后 Run 值被移除。
- Portable 从独立目录和不同 working directory 启动成功，且不会在 portable 目录创建 `config.toml`。
- 日志保留数量符合最多 5 个文件的约束，隐私模式扫描未发现 API Key、选中文字或 Clipboard 内容。

## 安装、升级与卸载验证

- 相同版本覆盖安装保留 Roaming 配置、Local 日志和 `Lexift/deepl-primary` Credential。
- 普通静默卸载清除 EXE、快捷方式、Run、Uninstall/Product registry 和当前安装路径的 tray history，同时保留配置、日志、Credential 和非安装路径 tray history。
- `uninstall.exe /S /DELETEUSERDATA` 清除 Roaming、Local、稳定 Credential、Run、Uninstall/Product registry。
- `/DELETEUSERDATA` 是显式 opt-in；省略时静默卸载和升级卸载继续保留用户数据。交互卸载仍由“删除用户数据”复选框控制。

## 仍需最终人工 Smoke Test

创建不可变的 `v0.1.0` Tag 前仍需在最终 CI Artifact 上确认：

- 主窗口关闭后进程、托盘和 Hotkey 继续运行，Tray Open/Quit 行为正确。
- 登录启动后主窗口不显示，Tray 与 Hotkey 可用。
- Chrome、Edge、VS Code、Notepad、Word 和 PDF Reader 的真实选择、翻译、Popup 全链路。
- 文本、图片和文件 Clipboard fallback 的桌面恢复行为。
- Popup 不抢焦点及多显示器/屏幕边缘定位。
- 从 CI Artifact 安装后完成一轮安装、启动、翻译和卸载。

在这些项目完成前不得创建或推送 `v0.1.0` Tag。
