# OCR 首次显示与区域选择重构验证

日期：2026-10-10。Windows x64，主屏 2560×1440 / 125%，右侧副屏 1920×1080 / 100%。

## 问题与修正

此前静态截图只验证最终画面，未覆盖首次可见瞬间。代码在取消监听中通过 `SetWindowLongPtrW` 临时移除边框，未刷新非客户区，也没有独立的首帧交接。

本次将原生准备独立为回调，加入 Winit 状态同步、非客户区刷新及 HWND 所属持续保护。冻结截图生成的暗色底图仅用于首次可见绘制，交接后释放并触发完整重绘。实施中还修正了反复托底覆盖局部重绘、导致操作提示迟出现的问题；不以灰色底图永久盖住界面来消除黑帧。

共用区域选择模型与展示宿主分离。源会话、请求、快照及实际窗口实例分别校验；关闭后旧输入及绘制回调不能更新新宿主。

## 自动化

- App 54、Core 125 项通过；日志 `target/region-selection-tests-final.log`。
- 最终 Platform 173、UI 365 项通过，包含真实 HWND 隐藏准备、后端样式改写、精确客户区、无激活与销毁；共用四方向／负坐标手势；部分准备失败整组销毁；准备取消、旧窗口回调和零面积。
- 最终 Platform / UI 日志：`target/region-selection-platform-ui-verified.log`。既有 ignored 实机测试不视为通过。
- Clippy：`target/region-selection-clippy-verified2.log`；Windows Release：`target/region-selection-release-verified.log`；格式与 diff 检查单独执行。

## 实机证据

临时脚本、录屏、关键帧及数值记录均保留于忽略的 `target/`。录制仅使用自建明亮测试底图；完整框选回归使用仓库内合成文字图片。

- 连续原生样式采样：`target/region-first-frame/first-visible-hwnd-0.json`、`first-visible-hwnd-1.json`。首次可见后客户区范围恒定，未出现标题栏／缩放框；不激活、TOOLWINDOW 和 APPWINDOW 排除保持成立。
- Desktop Duplication 录屏：`first-show-display-0.mp4`、`first-show-display-1.mp4`。录制请求为 120fps，**实际帧率和采样间隔以 `frame-analysis.json` 为准**，不将录制参数当作实际硬件逐帧覆盖保证。
- 最终录像使用 Release 程序，两个屏幕分别获得 326／323 个实际帧，约 65.3／64.7fps，最大采样间隔约 33.334ms；对应 HWND 可见样本为 3383／3400 条。执行日志为 `target/region-first-frame-release-probe.log` 和 `target/region-first-frame-release-analysis.log`。
- 每个实际解码帧检查顶部、右侧边缘及中央样本的 RGB，未出现黑色或白色空帧。保留首次变暗前、首次可见、下一帧及稳定画面的 PNG；人工核对整张关键帧无原生边框，操作提示最终正常呈现。
- 最终 Release 完整双屏回归：`target/region-native/record.json`、`target/region-native-release-probe.log`，覆盖四方向跨屏选区、Escape／右键／重复快捷键取消、OCR 原文和 DeepL 译文。独立查看实际 Alt+Tab 列表及打开 Start 后的任务栏截图，均没有 OCR 窗口入口；Shell 打开会按现有规则取消框选。

## 边界

- 用户随后从桌面启动 Release，出现资源下载确认。核对文件句柄最终路径后，发现此前由 Codex 启动的测试进程受 MSIX 文件系统重定向：资源实际位于 `Packages/OpenAI.Codex_2p2nqsd0c76g0/LocalCache/Local/Lexift/ocr/v1`。这些测试不能代表桌面启动时的资源发现结果。
- 已通过桌面 Shell 启动独立安装脚本，将本地构建并逐文件校验的资源原子安装到正常用户 `LocalAppData/Lexift/ocr/v1`；再次校验全部 SHA-256，并通过文件句柄确认最终物理路径不在 Codex 隔离目录。记录为 `target/ocr-desktop-install-result.json`。这仅解决本机资源缺失，不代表生产下载入口已发布，也不把资源确认框当作原生边框问题已复现或已排除的证据。
- 当前没有负坐标显示器，负坐标由共用手势、裁切及原生客户区回归覆盖。
- 录屏存在采样间隔，不能声称覆盖所有显示设备、显卡、远程桌面及每个物理扫描帧。当前环境没有观察到用户截图中的原生边框／黑帧。
- 不修改资源包、下载地址、Schema 或标注交互；生产资源发布状态沿用上一轮记录。
- 本轮构建目标为 `target/release/lexift.exe`；上一轮安装包和便携 ZIP 未重新打包，不能视为包含本轮修复。
