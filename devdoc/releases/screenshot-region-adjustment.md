# 截图选区调整反馈验证

> 鼠标模式的对象编辑及“选择对象”命名已被后续修正取代，当前行为以 [鼠标模式语义与穿透回归](mouse-mode-validation.md) 为准。本页保留当时的验证记录。

## 实现

- Core `RegionEditor` 统一提供命中结果与当前移动／缩放类型，输入与光标共用该规则，控制点优先于选区内部，已有对象优先于选区内部移动。
- 截图选区调整期间暂时隐藏工具条，保留组件、参数和收起状态，清理提示。松手、Escape 或捕获丢失后恢复；取消回退选区。显示任务校验截图身份、调整编号、工具条实例及标注窗口代次。
- 复用系统移动、八方向缩放光标和已有静止指针刷新，首次悬停不依赖额外鼠标移动。普通标注与贴图编辑不启用截图选区的隐藏逻辑。

## 自动化结果

- Core／UI 完整回归：154／398 项通过（UI 忽略 10 项），共 552 项。日志：`target/screenshot-adjust-tests.log`。
- 后续静止光标刷新及提示清理调整后，再次通过截图工具条流程测试和 7 项光标回归；日志：`target/screenshot-adjust-final-test.log`、`target/screenshot-adjust-cursor-tests.log`。
- 扩展现有测试覆盖移动、八个缩放控制点、按下未移动、松手恢复、Escape、捕获丢失、旧准备回调、对象命中优先级、离开区域以及绘制工具不显示选区移动光标。Core 同时覆盖 100%／125% 命中、负坐标、调整类型锁定及边界限制。
- 格式、差异空白检查、UI all-targets Clippy（`-D warnings`）、Slint 编译及 Windows Release 构建通过。最终构建日志：`target/screenshot-adjust-release-delivery.log`；Clippy 日志：`target/screenshot-adjust-clippy-final.log`。

## Windows Release 实机

运行项目 `target/release/lexift.exe`，通过真实鼠标输入和 HWND 枚举核对：

- 96 DPI（100%）与 120 DPI（125%）显示器上，移动和角点缩放期间工具条不可见，松手后恰好恢复一套。
- 系统游标句柄分别匹配四向移动及左上角斜向缩放光标。
- 选区从 100% 显示器拖到 125% 显示器并跨越边界，工具条正常隐藏和恢复。
- 再拖至桌面左边缘，选区受桌面范围限制，工具条恢复后位于工作区内。
- 结束后关闭测试截图和标注窗口，仅保留后台托盘。

桌面截图与辅助脚本在忽略目录 `target/screenshot-adjust/`：`moving-96.png`、`restored-96.png`、`moving-120.png`、`restored-120.png`、`edge-restored.png`、`native-check.ps1`。截图使用桌面合成结果；游标形状另由 `GetCursorInfo` 校验，截图不包含鼠标。

组合脚本最初复用游标信息结构，缩放句柄断言失败；改为重新创建该结构后，100%／125% 组合流程均通过。单独悬停／缩放检查也通过，该脚本问题不计为产品回归。

## 未覆盖范围

本轮实机未逐一检查全部八个缩放方向、浅深主题及真实捕获丢失；这些由对应自动化覆盖或沿用现有主题。未注入原生隐藏／显示失败，未重新检查系统任务栏及 Alt+Tab 外壳。短暂隐藏使用现有辅助窗口策略，不改变窗口角色或创建新窗口。

功能说明见 [截图模式](../features/screenshot-mode.md)。未新增配置、依赖、Port 或可见文案，未暂存、提交或推送。
