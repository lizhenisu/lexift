# 标注按钮延迟提示验证（2026-10-12）

## 变更与边界

为主工具条及参数面板纯图标按钮增加 1000ms 悬停提示，补齐 12 种语言名称及可访问名称。禁用按钮只开放悬停；操作仍按 enabled 判断。

Slint 1.18.1 Winit 内置 Tooltip 只能嵌入父窗口，不能在 54 DIP 高的工具条外显示，因此使用同视觉的按需辅助提示窗口。复用隐藏准备、被动工具条输入桥及窗口归属；鼠标穿透通过 Platform 的 HWND 禁用输入及所属实例保护独立注入，避免误注册为标注画布。

未更改字体扫描服务、Core、配置、依赖、画布捕获和参数事务。

## 自动化与渲染

- UI 单元／回归：375 通过，10 个既有性能测试按原规则忽略。包括工具菜单不触发字体加载、参数编辑、绘制和窗口生命周期回归。
- 新测试覆盖 999ms 不显示／1000ms 显示、移出、目标切换重新计时、旧目标取消不影响新目标、按下隐藏且不自动恢复、禁用点击不执行、退休实例和重开后的旧请求，以及负坐标、底部翻转与横向约束。
- 12 种语言 × 浅深主题 × 100%／125% 软件渲染共 48 张提示图；检查换行与内容尺寸。原始 BMP 及合成预览在 `target/annotation-hints/renders/`、`target/annotation-hints/render-sheet.png`。
- UI／Platform／App 全 target Clippy（`-D warnings`）、Slint 编译与格式检查通过。最终日志使用 `target/annotation-hint-{tests,clippy,release}-final.log`。

## 实机检查

最终使用 `target/release/lexift.exe`，核对进程物理路径。本机实际选择 Slint software renderer。

- 120 DPI（125%）检查禁用撤销、参数面板椭圆／数值按钮、工具条收起后的展开提示；96 DPI（100%）检查同一 HWND 跨屏拖动、展开与撤销提示。
- 6 次显示观测约 1020.8–1034.3ms；实机在 750ms 检查未显示，精确 999ms／1000ms 边界由可控时钟测试覆盖。测量包含隐藏创建和准备，不把 HWND 可见等同于最终画面，截图另在其后采样并人工检查。
- 提示显示前后前台 HWND 不变；提示禁用输入、WM_NCHITTEST 返回 HTTRANSPARENT，未进入任务栏或 Alt+Tab。实际 Shell 截图为 `target/annotation-hints/{alt-tab,taskbar}.png`。
- 移出、切目标、收起、关闭面板均隐藏并销毁旧提示；销毁可在后续事件循环完成，实测等待确认 IsWindow 为 false。关闭重开后，旧窗口和旧等待均未恢复。
- 第二屏右下角实际翻到上方并约束右边界；提示 HWND 为 `[4404,1002,4480,1030]`。主工具条尺寸和输入保持原样。
- 截图与元数据在 `target/annotation-hints/`：`native-125.png`、`native-100.png`、`panel-hint-125.png`、`numeric-hint-125.png`、`collapsed-hint-125.png`、`bottom-right-100.png`、`native-sequence.json`。实测结束已退出本轮自建进程。

## 未覆盖范围

- 12 种语言、浅深色均有软件渲染覆盖；原生双屏实测使用简体中文和浅色，未逐种语言／主题重复原生检查。
- 本机原生采用 software；未在另一台 GPU／femtovg 环境重复原生检查。标注 GPU 与软件绘制、文字、撤销及参数事务使用既有自动化回归，未在此次实机逐一绘制所有工具。
- 未执行辅助技术朗读实测或负坐标显示器实测；负坐标由纯坐标回归覆盖。

## 实机发现及修正

首轮实机在约 1026ms 观测到提示 HWND，但前台窗口改变且 客户区内容不可见，该轮不计为验收通过。原因分别是默认 Winit 创建属性会在首次 show 激活，以及 Winit `set_cursor_hittest(false)` 自动增加 WS_EX_LAYERED，影响当前 客户区呈现。

修正为复用公共隐藏创建作用域（active=false），并使用禁用输入、样式保护及 HTTRANSPARENT 的提示专属子类；不事后恢复焦点，不给 Slint 窗口增加 layered 样式。提示显示也不唤醒标注渲染器，防止仅悬停就分配 GPU 场景。销毁时移除子类。

移出实测还发现 Winit 会保留已显示组件：仅释放 Option 中的组件并不能销毁 HWND。提示改为使用既有窗口 Slot，沿用 hide、撤销输入和销毁顺序；最终复验必须断言旧 HWND 消失，不能仅检查 UI 状态已清空。Windows 输入保护回归 1 项通过；生命周期修正后的提示回归 4 项通过。

## 最终产物

- Windows Release 构建通过；最终日志 `target/annotation-hint-release-final-current.log`。
- SHA-256：`eed9e4a4479f0acf607a971dfa6c63add1015b33a75dcc2ce9d4c24d320ef169`。
- 最后清理修正后提示 4 项回归与 UI／Platform／App 全 target Clippy 再次通过，日志 `target/annotation-hint-lifecycle-tests.log`、`target/annotation-hint-clippy-final.log`；此前完整 UI 回归 375 通过、10 个既有性能测试忽略。
- 未暂存、提交或推送。
