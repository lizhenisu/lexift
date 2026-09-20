# Windows V0.1 Release Measurements

该文档记录正式 Release 构建的可复现产物指标。运行 `scripts/package-release.ps1` 后更新下表；启动耗时和内存必须在无调试器的干净 Windows 会话中测量。

| 指标 | V0.1.0 | 测量方法 |
| --- | ---: | --- |
| `lexift.exe` 大小 | 15,144,960 bytes | `Get-Item target/release/lexift.exe` |
| NSIS 安装器大小 | 7,418,316 bytes | `Get-Item dist/*-setup.exe` |
| Portable ZIP 大小 | 7,857,016 bytes | `Get-Item dist/*-portable.zip` |
| 冷启动到托盘 | 待桌面验收填入 | Windows Performance Recorder 或外部秒表，运行 `lexift.exe --background` |
| 后台稳定 Working Set | 待桌面验收填入 | 启动 30 秒后读取 `WorkingSet64` |

性能数据只用于版本间回归比较。测量时记录 Windows 版本、CPU、内存和是否已配置 DeepL 凭证。
