# 鼠标模式语义与穿透回归（2026-10-15）

## 当前行为

上一轮把鼠标模式扩展为“选择对象”属于错误行为，现已撤销：

- 箭头按钮、延迟提示及可访问名称统一为“鼠标操作”，全部 12 种语言同步。
- 普通标注穿透画布操作桌面；截图只允许移动、缩放选区，标注不阻挡选区命中；贴图标注穿透目标贴图，切回绘制工具恢复输入，其他贴图不受影响。
- 鼠标模式统一为 `EditScope::None`，清空对象选择、悬停、参数面板及控制点，拒绝对象指针和旧参数请求。恢复及新建编辑宿主也立即同步编辑范围，而非仅修改工具条状态。
- 对象编辑仍使用对应绘制工具的共用手势、参数、文字编辑及撤销；短暂选区调整隐藏工具条的逻辑保持。
- 贴图使用既有 App 注入的 Pin 窗口策略设置目标实例输入，未注册为画布，未新增 Port、依赖、crate 或配置。

## 自动化

- UI **399 项通过、10 项忽略**；Platform **192 项通过、94 项忽略**，共 591 项常规回归。日志：`target/mouse-mode-final-tests.log`。
- 初始化及恢复范围补齐后，再次通过 8 项截图／贴图相关回归，日志 `target/mouse-mode-screenshot-final.log`。
- 跨类型验证矩形、椭圆、荧光笔、箭头、文字、序号、马赛克和放大镜：切入鼠标模式后不可命中，旧参数请求被拒绝，对象和绘制默认值不变，未产生额外撤销步骤。
- 截图测试在标注覆盖处拖动选区，确认对象、选择状态及历史不变，工具条隐藏和恢复正常；滚轮不能修改对象参数。
- 贴图测试覆盖初始鼠标模式、过期指针及滚轮、切回绘制工具，以及三实例间输入隔离；恢复普通贴图后继续复用原拖动、缩放和关闭测试。
- 格式、Slint 编译、UI all-targets Clippy（`-D warnings`）及 Windows Release 构建通过。最终日志：`target/mouse-mode-clippy-delivery.log`、`target/mouse-mode-final-build.log`。

## 原生窗口验证

显式运行实际 App 的原生诊断回归，经过生产 Core／App 调度、Slint 窗口和 Platform 回调，验证多贴图拖动、截图内效果、进入贴图编辑、鼠标穿透、切回绘制及 Undo／关闭。此测试使用 **Debug 构建**，不宣称等同于 Release 手动点击验收。

首次运行检查出贴图初始化遗漏：界面为鼠标模式，领域编辑范围仍允许对象。修正初始化与恢复同步后，完整流程通过。

穿透由 `WindowFromPoint` 不命中目标 HWND 且 `WS_EX_TRANSPARENT` 已设置联合校验：

- 125% 主屏：贴图标注鼠标模式穿透；恢复后的普通标注画布穿透。
- 将编辑贴图移到 100% 副屏：DPI 更新后仍穿透，未被原生角色保护恢复为可输入。
- 切回绘制工具后，原生鼠标绘制实际新增对象，撤销有效；旧、新贴图继续可拖动及关闭，最终跟踪的 HWND 均销毁。

日志：`target/mouse-mode-native-final.log`、最终复跑 `target/mouse-mode-native-final-run.log`。前者记录 `MOUSE_MODE_PIN_SCALE=1.25`、`MOUSE_MODE_SECONDARY_SCALE=1`、`MOUSE_MODE_DESKTOP_PASSTHROUGH=true` 和 `PIN_REPRO_RESULT success=true`。

已查看窗口实际渲染快照：`target/mouse-mode/mouse-mode-edit.png`、`mouse-mode-secondary.png`。125% 与 100% 均没有对象控制点。原始 BMP 位于 `target/multi-pin/`。它们是原生窗口的 Slint 快照，不是桌面透明合成截图；PNG 转换没有改变像素内容。

## 未覆盖与工具限制

本轮 Computer Use 的窗口列表未暴露 Lexift 辅助窗口，未使用猜测句柄绕过访问。因此 Release 只完成构建和启动检查，未宣称完成该工具上的 Release 点击／穿透端到端流程。原生穿透结论来自上述真实 App 的 Debug 诊断，软件组件回归不替代该结论。

未重新实测所有绘制类型的桌面操作、系统任务栏／Alt+Tab、原生策略失败注入或 PNG／剪贴板输出。资源、配置、翻译、图片内容及普通贴图窗口行为未扩展。

当前功能说明见 [截图模式与贴图](../features/screenshot-mode.md)。历史 [选择对象模式回归](screenshot-object-selection.md) 中允许鼠标编辑对象的行为不再适用。本轮未暂存、提交或推送。
