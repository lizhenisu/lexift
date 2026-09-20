# Lexift

Lexift 是一款使用 Rust 与 Slint 构建的 Windows 桌面划词翻译工具。

## 快速开始

从 GitHub Releases 下载 `exe` 安装包，或下载 portable ZIP 解压后直接运行 `lexift.exe`。

首次使用：

1. 打开 Settings，在 DeepL API key 字段中保存 API Key。
2. 在任意应用中选中文字。
3. 按 `Alt + X` 获取并翻译选中内容。

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
