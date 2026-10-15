# 截图尺寸标签层级验证

> 独立标签窗口的方案已被 [截图尺寸同帧呈现](screenshot-same-frame.md) 替代：层级保护不能解决快速拖动时两条呈现路径的时间差。本页保留当时的验证记录。

日期：2026-10-15。

## 原因与修正

标签先于编辑画布创建，只应用了辅助窗口及鼠标穿透策略，没有绑定到标注画布，未参与其层级保护。新画布可能覆盖仍返回 `is_visible() == true` 的标签；组件可见断言不能证明桌面显示。

- 画布完成原生输入准备后，标签通过既有 App 注入的归属回调直接绑定到所属画布，参与现有窗口排序；不绑定到会隐藏的工具条。
- 归属更新仅发生在窗口准备或目标画布变化时，复用常态标签实例；不增加持续置顶轮询。
- 异步操作校验截图会话、标签实例、编辑代次及画布实例，使用有界重试；执行原生归属前释放编辑 Registry 借用。切回先前所属屏幕会取消尚未执行的旧归属请求。
- 画布销毁前清理标签、输入与归属延迟任务，避免保留已销毁原生窗口对应的组件。

## 自动化

- UI 402 项通过，10 项既有忽略；Platform 192 项通过，95 项忽略（包含本轮新增的桌面交互测试）。日志 `target/screenshot-label-order-tests.log`。
- 单独运行新增原生测试 `screenshot_size_label_stays_above_later_canvases` 通过：先显示标签，再创建画布；确认标签仍为可见后，绑定归属，再创建覆盖画布并隐藏／恢复工具条。检查真实 HWND 层级、owner、禁用输入、桌面像素、前台窗口及销毁清理。日志 `target/screenshot-label-order-native.log`。此项使用测试原生窗口，不能代替完整应用验收。
- UI 集成回归补充确认标签绑定到画布而非工具条、另一代次不能退休标签、旧归属回调不能绑定重开的画布。
- UI／Platform all-targets Clippy（`-D warnings`）通过：`target/screenshot-label-order-clippy.log`。

## Release 实机

- Windows Release 构建通过，耗时 4 分钟：`target/screenshot-label-order-release.log`。格式、差异空白和文档链接检查通过。
- 通过完整项目路径直接启动并核对进程：本轮 PID 29996，路径 `D:/MyDocuments/Projects/lexift/target/release/lexift.exe`。未使用安装版窗口作为验证对象。
- 两屏分别在 120 DPI（125%）和 96 DPI（100%）执行真实鼠标框选、松手、移动、缩放、Escape 取消调整、退出和重开。标签归属于截图画布，原生 Z 顺序位于全部截图画布上方；工具条隐藏时标签仍可见。
- 对比框选期间与松手后标签内部桌面像素，两种 DPI 的差异比例均为 0：`target/screenshot-label-order/check-120.log`、`check-96.log`。此检查同时验证实际像素，不以窗口可见标记替代显示结论。
- 两种 DPI 的 Escape 调整取消均恢复标签原位置及工具条；退出截图后用 `IsWindow` 检查记录的标签 HWND 和 owner 画布 HWND，均已销毁：`cancel-120.log`、`cancel-96.log`、`retired-120.log`、`retired-96.log`。
- 桌面实际截图已检查：`target/screenshot-label-order/selecting-{120,96}.png`、`released-{120,96}.png`、`moving-{120,96}.png`、`resizing-{120,96}.png`、`cancel-restored-{120,96}.png`。缩放期间尺寸由 `320 × 240 px` 更新为 `345 × 260 px`，标签持续显示；工具条仅在结束调整后恢复。
- 测试结束关闭了截图及标注窗口，保留项目新版在托盘运行。辅助脚本和证据均位于忽略的 `target/screenshot-label-order/`。

## 未覆盖范围

本轮 Release 实测使用当前深色主题；浅色主题沿用上一轮组件渲染验证，未再次切换系统／应用主题进行桌面实测。未手动操作任务栏和 Alt+Tab，也未热插拔显示器或在拖动中改变系统 DPI。原生测试验证窗口不激活，常规窗口角色回归同时通过；不将这些自动化结论等同于未执行的手工步骤。
