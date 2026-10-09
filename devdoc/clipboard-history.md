# 剪贴板历史（Windows）

划词工具条的第三个 SVG 按钮关闭工具条，并创建独立、不激活的历史窗口。历史窗口没有工具条 owner，关闭不会恢复旧工具条。窗口按辅助窗口策略排除任务栏和 Alt+Tab。

标题和记录状态区域支持按住左键拖动，关闭按钮不参与拖动。复用非激活窗口的原生鼠标捕获，在物理坐标中更新窗口位置；松手、捕获丢失或关闭窗口终止拖动，不新增后台轮询。条目区仍用于滚动、复制、删除和粘贴，窗口移动不会改变冻结的原输入目标。

拖动命中层覆盖从窗口顶部到标题分隔线的整个区域，与标题内容内边距独立；图标、数量徽标、顶部及左右留白均可拖动。仅关闭按钮的 32 × 32 逻辑像素点击区域排除，按钮上方、下方及右侧留白仍属于拖动区，悬停与拖动均保持普通箭头。

设置中的“清空剪贴板历史”先显示窗口内确认弹窗，说明清空不可撤销；默认聚焦“取消”，取消、关闭和 Escape 均不清空。只有明确确认才调用既有清空操作，记录开关不变。确认状态随切页或窗口销毁清除，重复确认不会重复派发。相关 UI 回归及多语言 / 浅深色 / 100%–125% 渲染产物位于 `target/history-clear-test.log` 和 `target/history-clear-snapshots/`。

## 视觉与操作反馈

历史窗口保留 340 × 420 的默认逻辑尺寸，单行顶栏使用 SVG 标识、记录状态、数量徽标和关闭按钮，移除可见标题但保留原生窗口名称及标题栏可访问名称。顶栏为 48px，内部 32px 高，整个 22px 数量徽标按行高垂直居中；列表和分隔线上移 24px，整条顶栏留白仍可拖动。条目为紧凑卡片，正文最多三行，复制与删除在右侧垂直居中。底部固定保留粘贴说明和设置入口，正文独立滚动；窄窗口将设置入口收为图标，保留可访问名称。

悬停、按下、复制等待、复制成功、删除确认和粘贴等待分别反馈。复制成功仍只在对应按钮显示 1.5 秒主题色对号；删除确认不改变正文预留宽度，避免条目抖动。粘贴期间对应条目显示进度，其余冲突操作禁用；失败在底部显示独立错误区域。首次启用、空历史和暂停记录各有明确状态，不改变记录、存储或输入目标规则。

软件渲染与点击回归覆盖 12 种语言、浅深色、100% / 125% 和窄窗口；等待状态、条目隔离、删除确认及设置清空确认回归通过。产物为 `target/history-polish-test.log`、`target/history-polish-clippy.log` 和 `target/history-polish-snapshots/`。

本轮 Core / App 历史回归 8 项、UI 回归 4 项、严格 UI Clippy、格式检查和 Windows Release 构建通过。最终 Release 在实际 96 / 120 DPI 显示器验证了复制及对号恢复、完整多行粘贴、删除二次确认、标题拖动、Escape、设置入口、重启恢复、只读 / 修饰键拦截及辅助窗口样式；截图已逐项核对，实际 Alt+Tab 无历史窗口条目。日志为 `target/history-polish-native-second.log`，窗口截图为 `target/polish-history-native-monitor-96.png`、`target/polish-copy-native-monitor-120.png` 和 `target/polish-delete-native-monitor-120.png`。验证结束退出测试进程并恢复原配置、历史文件和剪贴板。

首次实机流程显示已有历史，与脚本预期的首次启用状态不一致，止于启用断言；补充文件删除与路径检查后，第二轮完整流程通过。首轮记录 `target/history-polish-native.log` 保留，尚未确定其状态不一致原因，不声称已修复。深色、全部语言、等待 / 失败和极窄窗口的本轮验收使用软件渲染；实际应用与输入法的既有实测边界仍见下文。

## 记录与存储

- 首次打开须明确启用；配置保存成功后才注册 Windows 剪贴板变化通知，并读取当前文本一次。
- 普通配置只保存 `clipboard_history_enabled`、`clipboard_history_limit`；Schema 9 从旧配置迁移为关闭记录、50 条。
- 纯文本保留换行和空白；完全相同内容移到顶部。上限 10–200 条，单条 1 MiB UTF-8，总正文 16 MiB，超限淘汰最旧项。
- Windows 历史排除格式和 Lexift 凭证复制不进入历史。Alt+X 事务只排除自己拥有的剪贴板序列，并缓冲同时发生的用户复制。
- `%LOCALAPPDATA%/Lexift/clipboard-history.dat` 为版本化 DPAPI 用户级密文，后台串行原子替换。旧版本写任务不能覆盖删除、清空后的新版本。解密失败保留原文件，明确清空后才允许重新存储。
- 停止记录保留历史；退出撤销监听并完成队列中的最后写入。历史正文不写入配置或日志；Debug 仅显示条目 ID 与字节数。

## 粘贴和取消

Core 保存原窗口、原焦点控件的标识及独立会话；Platform 在后台用 UIA 确认可编辑、非密码、非只读，并记录运行时标识，不把 COM 对象传给 Core。

点击条目再次验证窗口、焦点及 UIA 标识，检查修饰键和取消标记，写入完整文本并发送一次 Ctrl+V。不会激活原窗口、定位鼠标、重试或恢复旧剪贴板。输入目标无法确认时不注入；失败保持面板并提示。

目标预检区分检查中、可编辑和不支持。检查失败不在打开时显示常驻错误；用户点击现存条目尝试粘贴，才产生按会话及尝试编号隔离的“原选区不是可确认的可编辑输入框”Toast，不生成剪贴板写入、Ctrl+V 或保存命令。检查中阻止粘贴，明确不支持时允许点击获取原因；复制和删除继续独立可用。

该 Toast 复用 Settings 的视觉组件，覆盖在列表底部和操作栏上方，不改变列表高度，5 秒消失，可手动关闭，重复点击重新计时且不堆叠。窄 / 矮窗口下限制提示高度并允许内部滚动，提示正文消耗点击，避免穿透粘贴下面的条目。关闭窗口停止所属计时器，旧窗口回调不会影响新会话。其他错误保留原反馈方式，复制成功仍使用按钮对号。

关闭、Escape、外部点击、新划词或输入焦点变化会结束会话。准备、输入投递及后台结果按窗口实例、会话与请求编号隔离。窗口关闭时停止自己的计时器、回调和原生监听。

## 验证记录（2026-10-09）

- Core / Config / Platform / UI / App：693 项通过；另有 94 项既有交互测试默认忽略。Slint 编译、相关 crate 的全部 target 严格 Clippy（`-D warnings`）及 Windows Release 构建通过。
- 单独执行原生通知测试通过：初始文本读取、普通复制、凭证复制排除、停止监听，以及大容量剪贴板分配中有效短文本的读取。
- 自动化覆盖数量及字节边界、精确去重、损坏文件不覆盖、配置迁移、串行写版本屏障、取消、旧窗口回调及负坐标 / 小工作区定位。软件渲染快照覆盖全部 12 种语言、浅深色、100% / 125%、末条滚动访问和窄窗口首次启用说明 / 按钮的可达性。
- 最新 Release 在受控 Windows 文本输入框实测：第三个按钮立即销毁工具条、面板不激活、原选区被完整多行中文文本替换、Escape 关闭、设置入口、加密持久化和异常退出后的重启恢复。修饰键未释放及只读输入均拒绝注入，错误保留在面板中。
- 两块实际显示器分别为 96 DPI（100%）与 120 DPI（125%）；已保存窗口截图并检查。HWND 保持 `TOOLWINDOW` 且没有 `APPWINDOW`。实际 Alt+Tab 截图确认历史面板没有条目，Settings 打开时正常出现。
- 受控验证结束恢复原配置、加密历史文件和系统剪贴板，并退出测试创建的进程；不安装任何服务或模型。

### 本地验证产物

产物位于忽略目录 `target/`：

- `history-tests-final.log`、`history-clippy-final.log`、`history-release-final.log`
- `history-native-notifications.log`、`history-native-extended.log`
- `history-native-toolbar.png`、`history-native-recorded.png`、`history-native-settings-card.png`
- `history-native-monitor-96.png`、`history-native-monitor-120.png`
- `history-native-modifier-blocked.png`、`history-native-readonly.png`、`history-native-restart.png`
- `history-snapshots/`：多语言、主题、缩放与窄窗口软件渲染快照。

### 实测边界

未逐一实测记事本、浏览器输入框和 VS Code；受控输入框结果不代表所有 UIA 编辑器均支持，无法可靠确认可编辑状态时明确拒绝粘贴。中文 IME、Ctrl+X 和 Alt+X 的真实应用组合未补充交互实测；Alt+X 并发序列排除由回归测试覆盖。任务栏处于自动隐藏状态，本轮独立 Shell 图像验收覆盖实际 Alt+Tab，任务栏排除检查以 HWND 策略为证据。正常退出的最终写入由运行时 shutdown 测试覆盖，实机重启测试使用异常终止后的最后成功落盘版本。

### 标题拖动补充验证

历史窗口 UI 渲染 / 点击、负坐标与 DPI 移动计算、旧实例回调失效回归通过；UI 严格 Clippy、Slint 编译、格式检查和 Windows Release 构建通过。实际 96 / 120 DPI 分别拖动 90 × 50 个物理像素，检查位置、松手停止、前台焦点及 `NOACTIVATE` / `TOOLWINDOW` / 无 `APPWINDOW` 样式，随后复制、删除确认正常。截图为 `target/history-drag-native-96.png`、`target/history-drag-native-120.png`；通过日志为 `target/history-drag-native-second.log`。

首次完整流程在尚未开始拖动时出现一次既有行点击未触发，记录于 `target/history-drag-native.log`；第二轮通过，尚未确认该偶发现象的原因，不将其当作已修复。跨屏拖动过程和捕获被其他程序抢走未补充实机测试；窗口关闭取消及负坐标由回归覆盖。

### 整条标题栏命中修正

顶部、左右边缘、数量徽标和关闭按钮四周共 10 个位置，分别在实际 96 / 120 DPI 显示器验证拖动：位移正确、松手停止、普通箭头、不改变前台焦点和辅助窗口样式。随后复制、删除确认、关闭和原选区粘贴通过，测试结束恢复原配置、历史文件及剪贴板并退出测试进程。UI 4 项相关回归（含 100% / 125% 命中矩阵）、Slint 编译、严格 UI Clippy、格式检查和 Windows Release 构建通过。产物为 `target/history-header-test.log`、`target/history-header-native.log`、`target/history-header-clippy.log`、`target/history-header-release.log` 和 `target/header-history-expanded-title-{96,120}.png`；未额外扩大跨屏拖动及捕获丢失的实机验证范围。

### 紧凑顶栏补充验证

移除可见标题并压缩顶栏后，4 项相关 UI 回归、Slint 编译、严格 UI Clippy、格式检查及 Windows Release 构建通过。软件渲染覆盖 12 种语言的记录 / 暂停 / 粘贴状态、浅深色、100% / 125%、首次启用、空列表及窄窗口。实际 96 / 120 DPI 截图分别核对了两条记录时整个数量徽标的行内垂直居中；每块屏幕 10 个位置的拖动、光标、释放、焦点与辅助窗口样式，以及复制、删除确认、关闭和原选区粘贴均通过。

产物为 `target/history-compact-test.log`、`target/history-compact-clippy.log`、`target/history-compact-release.log`、`target/history-compact-snapshots/`、`target/history-compact-native-isolated.log` 和 `target/compact-history-expanded-title-{96,120}.png`。首轮实机脚本在启动应用前的历史文件重置检查中止，记录为 `target/history-compact-native.log`；后续使用子进程独立的历史数据目录完成验证，不将重置失败算作产品验证通过。测试结束恢复原配置与剪贴板并退出测试进程；深色和全部语言继续使用软件渲染验收。

### 不可编辑选区 Toast 验证

Core 历史回归 8 项、UI 历史回归 5 项、Settings 回归 14 项及 App 历史存储回归 1 项通过（部分筛选重复执行相同测试）。新增回归断言预检失败无常驻提示，点击才反馈且不产生系统操作命令；用受控时钟验证 5 秒消失、重复点击重新计时、手动关闭、同一状态同步不重现提示以及旧会话回调失效。Core / UI / App 全部 target 严格 Clippy、格式检查和最终 Windows Release 构建通过。

最终 Release 在实际 96 / 120 DPI 的受控只读输入框验证打开无提示、点击显示、重复点击后超过首个期限仍显示、最终消失、手动关闭和关闭重开无旧提示；输入正文、剪贴板序列及前台焦点均保持不变。复制和删除确认继续可用，恢复可编辑目标后正常替换原选区。截图已核对，验证结束退出测试进程、恢复原配置和剪贴板；测试历史使用隔离目录。浅深色、12 种语言及 200 × 260 极窄 / 矮窗口的提示换行和内部滚动使用软件渲染验证，未扩大浏览器 / VS Code 等应用的实测范围。

产物为 `target/history-toast-{core-test,ui-test,app-test}.log`、`target/history-toast-settings-test-final.log`、`target/history-toast-clippy-final.log`、`target/history-toast-release-final.log`、`target/history-toast-snapshots/`、`target/history-toast-native.log` 和 `target/toast-target-{before-click,after-click,repeat-alive,expired,manually-dismissed,reopened-silent}-{96,120}.png`。
