# Windows V0.1 Release Measurements

该文档记录正式 Release 构建的可复现产物指标。运行 `scripts/package-release.ps1` 后更新下表；启动耗时和内存必须在无调试器的干净 Windows 会话中测量。下表及后续 A/B 数据是历史测量，不代表当前双渲染器构建。

当前双渲染器构建（2026-09-25）：`cargo build --release -p lexift-app` 生成的 `lexift.exe` 为 **16,524,800 bytes**；单屏 Intel 主显示器启动时选软件渲染，其他配置选 FemtoVG。软件渲染路径保留原有窗口补绘。此前单 FemtoVG 构建为 15,493,632 bytes，已安装的软件渲染版为 16,012,800 bytes。这些仅是文件大小；新构建的实际缩放流畅度、跨屏画面、CPU/GPU 占用和安装包体积仍需现场测量。

| 指标 | V0.1.0 | 测量方法 |
| --- | ---: | --- |
| `lexift.exe` 大小 | 15,144,960 bytes | `Get-Item target/release/lexift.exe` |
| NSIS 安装器大小 | 7,418,316 bytes | `Get-Item dist/*-setup.exe` |
| Portable ZIP 大小 | 7,857,016 bytes | `Get-Item dist/*-portable.zip` |
| 冷启动到托盘 | 待桌面验收填入 | Windows Performance Recorder 或外部秒表，运行 `lexift.exe --background` |
| 后台稳定 Working Set | 待桌面验收填入 | 启动 30 秒后读取 `WorkingSet64` |

性能数据只用于版本间回归比较。测量时记录 Windows 版本、CPU、内存和是否已配置 DeepL 凭证。

## 渲染器体积对照（2026-09-24）

提交 `bcab46f` 将 Slint 渲染器由 FemtoVG 切换为 Skia OpenGL。同源 A/B Release 构建保存在 `dist/renderer-ab/`：FemtoVG 的 `lexift.exe` 为 15,490,560 bytes，Skia OpenGL 为 23,897,600 bytes，增长 8,407,040 bytes（约 8.02 MiB）。对应 Portable ZIP 分别为 7,929,674 和 11,718,314 bytes。该增长主要来自渲染器依赖和链接产物，不是 popup 的 Slint 界面代码。这是当时的渲染器取舍；当前默认值见文首。

体积比较必须区分未压缩的 `lexift.exe`、Portable ZIP 和 NSIS 安装器。仓库现有 `dist` 目录中的 NSIS 安装器为 10,223,080 bytes（约 9.75 MiB）；用户观察到的约 22 MB 与 Skia exe 体积更接近，不应写作安装器的实测值。上方 V0.1.0 表格是历史基线，不代表当前 Skia 构建。

## Intel 核显 A/B 后的正式渲染器（2026-09-24）

同机、同尺寸的慢速大幅缩放中，Skia OpenGL 的 Settings GPU 0 3D 峰值为 85.7%，FemtoVG 为 80.4%；软件渲染版没有测到 Lexift 进程的 GPU Engine 非零样本，Settings CPU 峰值为整机的 0.38%。测试者确认软件版 popup 和 Settings 的外观及拖拽手感正常，因此正式构建改用软件渲染。详情、采样限制和未覆盖的跨 DPI 检查见 `devdoc/resize-performance.md`。

切换后的 `cargo build --release -p lexift-app` 生成 `lexift.exe` 为 **16,026,112 bytes**；同一构建通过 `cargo packager --formats nsis` 生成的安装器为 **7,533,474 bytes**。相比切换前 Skia 的 23,919,616-byte exe 和 10,237,463-byte NSIS 安装器，分别减少 **7,893,504 bytes** 和 **2,703,989 bytes**。这两个安装包均为本地未签名诊断产物；不要把历史表格中的 V0.1.0 数值当作本轮基线。

本次 popup 顶边与跨屏修复后的 `cargo build --release -p lexift-app` 生成 exe 为 23,900,160 bytes，相比同源 Skia A/B 构建增加 2,560 bytes。通过 `cargo packager --release --formats nsis` 输出到临时目录的 NSIS 安装器为 10,228,849 bytes，相比仓库现有安装器增加 5,769 bytes；该临时安装器未签名，且与仓库现有安装器可能存在签名及构建时点差异，因此只作为回归参考，不替换正式发布产物。

随后固定 popup 缩放期间内容布局的 Release exe 为 23,900,672 bytes，比上次构建增加 512 bytes；本轮未重新制作 NSIS 安装器。

上述两轮体积都是在 Skia OpenGL 仍为默认渲染器、且 popup/Settings 缩放带 80 ms 绘制门与布局追赶的版本上测得的。2026-09-25 删除了这些缩放期的绘制优化，并把渲染器默认值改为软件渲染；本次改动后的 exe 与安装器体积需要重新测量后写入本节，历史数值不要与软件渲染构建直接相减。

## 无窗口工作集修剪实验（2026-09-23）

在 Windows 11 Pro build 26200、Ryzen 9 7945HX、16 GiB 内存的当前主机上，以 `GPU Process Memory` 计数器记录一轮窗口生命周期：主窗口约 23.5 MiB GPU committed；打开 Settings 后约 48.6 MiB；销毁 Settings 后回落至约 25.2 MiB；销毁主窗口后回落至 12.3 MiB。重开主窗口后回升至约 23.5 MiB。这证实每个 Slint 窗口的 Skia/OpenGL Renderer 和 surface 随窗口销毁，重建窗口时 GPU 资源重新分配；无窗口仍有约 12.3 MiB 的 GPU 进程基线。

随后无窗口闲置超过 2 分钟，`Win32_PerfFormattedData_PerfProc_Process` 中 `WorkingSetPrivate` 从约 62.9 MiB 降至约 1.05 MiB，`WorkingSet` 降至约 2.83 MiB；`PrivateBytes` 仍约 80.4 MiB。再次通过第二实例打开主窗口成功，私有工作集回升至约 8.65 MiB，GPU committed 回升至 23.5 MiB。另一轮测量中，修剪后的 PrivateBytes 约 58.2 MiB，说明该值随启动状态而变，不会被工作集修剪回收。

这说明两分钟策略回收的是驻留页，不会把进程的私有提交量恢复到启动基线。此实验的 `EmptyWorkingSet` 调用是在窗口级 Skia/OpenGL 资源已随组件销毁之后执行的；它不是渲染器卸载手段。实际数值会受 Windows 内存压力、字体缓存和驱动影响，不能视为每台机器的固定值。
