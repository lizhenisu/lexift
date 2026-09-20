# Lexift

Lexift 是一款使用 Rust 与 Slint 构建的 Windows 桌面划词翻译工具。

## 快速开始

从 [GitHub Releases](https://github.com/lizhenisu/lexift/releases) 下载 Windows `exe` 安装包，或下载 portable ZIP 解压后直接运行 `lexift.exe`。

首次使用：

1. 打开 Settings，在 DeepL API key 字段中保存 API Key。
2. 在任意应用中选中文字。
3. 按 `Alt + X` 获取并翻译选中内容。

快捷键可在 Settings 的 Shortcut 项中点击 **Change** 后修改，捕获到有效组合后会自动保存。目标语言和开机启动设置也会即时保存。

关闭主窗口只会把 Lexift 隐藏到系统托盘，划词翻译仍会继续工作。通过托盘菜单可以重新打开主窗口、打开 Settings 或完全退出 Lexift。

## 开发

```console
cargo run
```

M1 Mock 演示：

```console
cargo run --features m1-demo
```

## 许可证

[GPL-3.0](LICENSE)
