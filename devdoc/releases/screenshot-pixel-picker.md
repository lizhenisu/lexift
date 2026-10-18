# 截图像素放大镜与取色验证

日期：2026-10-18。起点：`bf1230b`；本轮修改未暂存、提交或推送。范围：首次框选与截图鼠标模式的冻结帧取色，不增加实时采集、HDR、颜色格式切换或全局快捷键。

## 实现

- Core `screen_snapshot::sample_pixels` 返回 15×9 物理像素邻域、中心坐标与 RGB；负坐标、显示器间隙、缓冲区边界、透明度及大写 HEX 均由纯函数处理。
- 独立颜色请求编号与完成事件，不占用图片输出编号。App 使用后台既有 ClipboardPort，取消未执行任务，并以服务串行门防止颜色写入覆盖随后输出的图片。
- 首次框选使用 Slint 子组件；编辑阶段使用按语言、主题和 DPI 缓存的静态外壳、有限字符字形与冻结网格，合成进现有交互帧。底图和标注缓存继续复用；输出不含放大镜。
- Windows 在真正完成首帧交接后发送所属窗口视图事件，UI 校验前台、会话、窗口实例与输入宿主，尝试一次激活。截图编辑的 Slint 输入树在隐藏准备中初始化；没有持续抢焦点、额外输入钩子或独立放大镜窗口。
- 旧屏幕迟到的离开事件不能清除新屏幕采样。结果提示仅持续约一秒且绑定原采样点，退出释放采样引用、缓存、计时与回调。

## 自动化

| 项目 | 结果与证据（忽略目录） |
| --- | --- |
| Core | 163 通过；`target/screenshot-pixel-core.log` |
| UI 全部库回归 | 442 通过，11 忽略；`target/screenshot-pixel-ui-final.log` |
| App 截图相关 | 5 通过；后台线程、服务门前取消、失败结果及既有输出；`target/screenshot-pixel-app-final.log` |
| Platform 首帧与窗口策略 | 3 通过；原生 HWND 上可控制交接时机，同一实例只通知一次；`target/screenshot-pixel-platform-frame.log` |
| Platform Popup／输入 | 24 通过，2 忽略；`target/screenshot-pixel-platform.log` |
| Slint、国际化与主题 | UI 编译通过；12 语言 × 浅深主题 × 100%／125% 共 48 份组件渲染，`target/screenshot-pixel/render/` |
| 格式与 Clippy | `cargo fmt --all --check` 与 Core／UI／App／Platform `--all-targets -- -D warnings` 通过；`target/screenshot-pixel-format.log`、`target/screenshot-pixel-clippy-verified.log` |
| Windows Release | 最终构建通过（4m38s）；`target/screenshot-pixel-release-final.log` |

UI 首次并发跑出现既有划词工具条的计时断言失败（DPI 变化后显示次数），单线程全套复跑通过；未修改该无关模块。11 项 UI 与 2 项 Popup 忽略项不计入已通过实机覆盖。

同帧回归覆盖延迟预览、冻结采样与位置、底图缓存不失效、旧结果隔离及干净输出；键盘回归覆盖 C 重复、拖动期间复制、绘制与外部前台屏蔽。原生首帧回归额外覆盖错误宿主、前台改变、过期会话、主动点击重试及 OCR 不激活。

## 原生实测

夹具为忽略目录中的已知像素图：底色 `#123456`，两个屏幕各有一组 15×9 逐格变化的色块，中心 `#777488`。实际显示器分别为 2560×1440／125% 与 1920×1080／100%，物理原点分别为 `(0,0)` 与 `(2560,0)`。

Debug 完整流程已通过：首次框选与编辑阶段双屏取色、四次 135 格逐像素核对、拖动中 C、八个工作区角落、绘制模式屏蔽、新增矩形不参与采样、400×400 图像输出与退出重开。证据为 `target/screenshot-pixel-debug-native-full.log` 及 `target/screenshot-pixel/` 中窗口记录和桌面截图。

最终 Release 完整流程通过，启动的进程由原生脚本校验为项目 `target/release/lexift.exe`，构建文件为 27,305,472 字节，SHA-256 为 `C1ACFE625C9511C46F7D515DA762FE54FEE7977F282E4CEB219FA121BA10C4F2`，详见 `target/screenshot-pixel/binary.json`。测试后关闭本次启动的应用与夹具进程。

- 首次框选和编辑各在两屏核对 135 格；再进行 40 次跨屏快速往返，最终位置额外核对 135 格。C 复制文本与冻结夹具逐项一致。
- 两屏共八个工作区角落均复制到 `#123456`；显示器单份呈现、面板翻转与夹紧保存为实际桌面截图。
- 矩形工具被实际选中并绘制，C 未复制颜色；切回鼠标后在矩形边框上取得底图 `#123456`。
- 导出图片实际为 400×400。位于预览放大镜覆盖范围内的 11,500 个底图像素全部保持原色（混入 0），同时保留 3,130 个矩形描边像素。见 `target/screenshot-pixel/output-pixels.json` 与 `output.png`。
- 输出后重开截图，首次框选 C 再次成功；Escape 退出。源窗口位置、DPI、归属、前台及扩展样式保存于 `windows.jsonl`，主要辅助窗口未出现 WS_EX_APPWINDOW。

Release 日志：`target/screenshot-pixel-release-native-full.log`；结果：`target/screenshot-pixel/native-result.txt`；截图：该目录下的 `select-125.png`、`select-100.png`、`edit-125.png`、`edit-100.png`、`rapid-final.png`、`edge-*.png`、`tool-selected.png`、`draw-picker-hidden.png`、`annotation-excluded.png` 和 `output-clean-before.png`。网格局部证据为对应 `*-grid.png`。这些生成文件不进入版本控制。

## 未覆盖范围

首次激活后顶部白条的后续修复与专项逐像素检查见 [顶部白条修复验证](screenshot-whitebar.md)；本记录的原有网格检查未覆盖该问题。

- 实机桌面没有负坐标显示器或屏幕间隙；这两项由 Core／UI 夹具覆盖。
- 12 种语言、浅深主题的完整矩阵属于组件渲染；原生双屏流程使用当前中文主题，不能等同于全部语言的原生测试。
- 剪贴板失败、晚到完成、取消写入由可控制服务的自动化覆盖，未强制锁死系统剪贴板。
- 未穷尽全部绘制工具、文字输入、菜单与尺寸面板的原生交错；这些入口的输入屏蔽由 UI 回归覆盖。
- 本轮原生流程没有逐一重跑八方向选区缩放及键盘微调的全部组合；相关行为以本轮全套 Core／UI 回归及既有专项验证为依据。
- 未对 HDR、色彩配置文件、非 Windows 或第三块显示器作出验证结论。
