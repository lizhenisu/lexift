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

## 无窗口工作集修剪实验（2026-09-23）

在 Windows 11 Pro build 26200、Ryzen 9 7945HX、16 GiB 内存的当前主机上，以 `GPU Process Memory` 计数器记录一轮窗口生命周期：主窗口约 23.5 MiB GPU committed；打开 Settings 后约 48.6 MiB；销毁 Settings 后回落至约 25.2 MiB；销毁主窗口后回落至 12.3 MiB。重开主窗口后回升至约 23.5 MiB。这证实每个 Slint 窗口的 Skia/OpenGL Renderer 和 surface 随窗口销毁，重建窗口时 GPU 资源重新分配；无窗口仍有约 12.3 MiB 的 GPU 进程基线。

随后无窗口闲置超过 2 分钟，`Win32_PerfFormattedData_PerfProc_Process` 中 `WorkingSetPrivate` 从约 62.9 MiB 降至约 1.05 MiB，`WorkingSet` 降至约 2.83 MiB；`PrivateBytes` 仍约 80.4 MiB。再次通过第二实例打开主窗口成功，私有工作集回升至约 8.65 MiB，GPU committed 回升至 23.5 MiB。另一轮测量中，修剪后的 PrivateBytes 约 58.2 MiB，说明该值随启动状态而变，不会被工作集修剪回收。

这说明两分钟策略回收的是驻留页，不会把进程的私有提交量恢复到启动基线。此实验的 `EmptyWorkingSet` 调用是在窗口级 Skia/OpenGL 资源已随组件销毁之后执行的；它不是渲染器卸载手段。实际数值会受 Windows 内存压力、字体缓存和驱动影响，不能视为每台机器的固定值。
