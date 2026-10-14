# 贴图菜单与恢复行为验证（2026-10-14）

## 实现

- 截图取消、成功输出及准备失败恢复后进入“选择对象”，保留原文档、工具默认参数和撤销历史。
- 移除贴图右上角关闭组件。五项内置上下文菜单，蓝色光晕与置顶默认开启，实例间独立、不持久化。
- 绘制工具下穿透，选择对象或关闭标注时可交互。Pin 输入策略不会将贴图注册成标注采集排除窗口；取消置顶后不会被层级排序重新置顶。
- Core 管理独立贴图输出，App 后台复用已有图像剪贴板和 PNG 保存能力；关闭取消任务。图像共享引用在窗口和任务结束后释放。
- 12 种语言增加菜单文案；无新增依赖、配置、crate 或 Port。

## 自动化与原生证据

证据均在忽略的 `target/`，没有将真实截图或正文加入源码。

- `pin-menu-tests.log`：相关 Core／Platform／App／UI 回归。覆盖重复请求、过期结果、关闭取消、源像素保持、独立窗口状态、鼠标穿透样式保护及无关闭按钮。
- `pin-menu-clippy.log`：相关 crate 全目标 Clippy，`-D warnings`。
- `pin-menu-native.log`：真实 App Debug 原生回归；两个屏幕，125% 主屏上进行贴图交互。冻结截图内绘制矩形、聚光灯、马赛克后贴图；恢复原标注，在其他区域绘制，再次截图贴图。新旧贴图各由实际鼠标拖动 40px，右键打开所属系统菜单，Escape 只关闭菜单；键盘选择阴影、置顶并核对 Slint 属性和实际 HWND 扩展样式。恢复置顶后用 Escape 依次关闭两图，检查实例清零及实际 HWND 销毁。
- `pin-menu/renders/`：浅深主题、100%／125% 软件渲染截图；像素断言验证蓝色光晕实际可见和关闭后消失。Slint 软件渲染器未实现 drop-shadow 的发现已体现在实现中。
- `multi-pin/native-old-after-new-moved.bmp` 与 `multi-pin/native-shadow-off.bmp`：真实 App 贴图渲染。实际菜单不是软件渲染截图，原生命中与菜单归属记录以 native 日志为准。

原生回归入口仍为设置环境变量 `LEXIFT_NATIVE_PIN_REPRO=1` 后显式运行 App 的 `native_pin_roundtrip_exact_effects_sequence` 忽略测试。该回归会短暂移动鼠标到本进程已核对的贴图；不会改写用户剪贴板或选择真实保存文件。真实 Windows 图像剪贴板与 PNG 编码复用现有能力，新增输出路径使用 Port 夹具检查源像素和取消。

## 边界

原生完整流程来自 Debug App，不能表述为最终 Release 完整验收。尚未实测：跨屏拖动时所有 DPI 过渡、负坐标屏幕、全部 12 种语言的原生菜单画面、真实保存对话框的全部取消路径、实际任务栏／Alt+Tab 人工检查及显存测量。最终结果见下方。

保留已有改动及参考图片；未暂存、提交或推送。

## 最终结果

- Core 152 项、Platform 193 项、App 61 项、UI 396 项通过，共 802 项；原有忽略项分别为 0、94、1、10。显式原生 App 回归另行通过（29.38s）。
- 相关四个 crate 全目标 Clippy（`-D warnings`）、Slint 编译、格式检查及标准 Git 差异空白检查通过。
- Windows Release 构建通过（3m 58s），`target/release/lexift.exe`，26,643,456 bytes；仅更新项目构建产物，未替换安装目录或发布包。
- 原生最终证据为 `target/pin-menu-native-final.log`；蓝色光晕截图为 `target/pin-menu/native-glow-final.png` 和 `target/pin-menu/renders/glow-light-125-final.png`。原生回归包含实际菜单勾选操作、取消置顶的 HWND 样式、绘制状态下画布命中、菜单 Escape 以及多贴图拖动／销毁。

已从项目绝对路径启动新版，确认 PID 41012。未暂存、提交或推送。

## 后续调整：仅保留光晕

删除贴图内容容器及覆盖层的两处实线边框。保留原有光晕、阴影开关和固定外边距；悬停不再改变边缘装饰，关闭阴影后悬停也没有边框。输入、窗口定位及原始图像输出未修改。

贴图相关 5 项 UI 测试通过，包含浅深主题、100%／125% 软件渲染。逐像素比较悬停前、悬停中和移出后画面一致，检查图像左右边缘没有覆盖线，以及阴影开启可见、关闭消失。已查看实际组件渲染的 PNG；截图和测试日志位于忽略的 `target/pin-border/renders/` 与 `target/pin-border-tests.log`。格式检查与差异空白检查通过。

本次渲染验证不代表桌面合成、跨屏输入或任务栏／Alt+Tab 的重新实测；这些原生流程未修改。

UI 全目标 Clippy（`-D warnings`）及 Slint 编译通过；Windows Release 构建通过（4m 03s），产物为 `target/release/lexift.exe`。日志分别为 `target/pin-border-clippy.log` 与 `target/pin-border-release.log`。未替换安装版或发布包，未暂存、提交或推送。

## 后续调整：柔和高斯光晕

原 PinGlow 的逐层 1px 描边仍产生同心框线，因此替换为共享的 33×33 高斯透明遮罩，九宫格拉伸并以主题蓝色着色。保留 8px 外边距，不新增 HWND、依赖、配置或文案；未修改输入和输出业务。静态 PNG 为 532 bytes，生成脚本可复现相同字节。

- 贴图相关 5 项 UI 回归通过。渲染覆盖浅深主题、100%／125%、横向与竖向尺寸、悬停与移出、阴影开关；新增亮度向内单调增强和外缘透明断言，保留无图像覆盖线及悬停前后像素一致断言。
- 实际组件 RGBA 渲染分别合成到深浅背景，已查看截图；与桌面实测区分。证据为 `target/pin-soft-glow/renders/` 和 `target/pin-soft-glow-tests.log`。
- UI 全目标 Clippy（`-D warnings`）、Slint 编译、格式与差异空白检查通过，日志在 `target/pin-soft-glow-*.log`。
- Windows Release 构建通过（4m 04s），26,635,264 bytes。实际启动路径为 `D:/MyDocuments/Projects/lexift/target/release/lexift.exe`，PID 28428。未替换安装版或发布包。
- 使用实际 Release，从标注工具条进入截图、物理框选、贴图，分别在 96 DPI 与 120 DPI 屏幕验证。100% 贴图 HWND 1048912，125% 贴图 HWND 1442996；深浅背景由临时测试窗口提供，截图通过桌面采集而非组件 snapshot 获取。已查看四张桌面截图，直边无多重描边、四角平滑。证据为 `target/pin-soft-glow/release-desktop-{100,125}-{dark,light}.png`；只存入忽略目录。
- 两张测试贴图均通过点击后 Escape 关闭，随后退出测试标注；枚举确认仅后台辅助窗口仍存在。Release 留在托盘运行。临时脚本 `target/pin-soft-glow-native.ps1` 在每次点击前校验目标 HWND 属于该进程且为实际鼠标命中窗口。

本次未重新完整实测跨屏拖动、原生菜单全部操作、实际任务栏／Alt+Tab 或其他平台；没有将软件渲染截图当作这些流程的验收。保留全部已有改动，未暂存、提交或推送。
