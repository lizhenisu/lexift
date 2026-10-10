# Windows 托盘菜单

## 当前实现

- Platform 保留通知区图标、左键动作、Explorer 的 `TaskbarCreated` 恢复处理。通过 Core 的 `TrayMenuRequest` 将物理屏幕锚点和键盘/鼠标来源交给 App 转发。
- Explorer 的键盘菜单也会发送右键通知，因此在首次通知时区分键盘输入；鼠标坐标取自右键通知，键盘位置通过 `Shell_NotifyIconGetRect` 获取。不依赖 `WM_CONTEXTMENU` 的未定义坐标参数。
- UI 按需创建 `TrayMenuWindow`，以同一 PO 词条目录提供文字，以 Colors 和 MenuMetrics 提供主题与布局。实际文字测量决定宽度，物理尺寸按窗口 DPI 换算后约束在工作区内。
- 菜单独立获得焦点，不打开主窗口。支持 Up/Down、Home/End、Enter/Space、Escape、名称首字符匹配；唯一匹配直接执行，重复匹配循环选择。
- 外部点击/失去焦点关闭菜单；仅显式取消且菜单仍拥有焦点时请求 `NIM_SETFOCUS`。选择动作先销毁菜单再发送现有 AppEvent。
- 每次打开使用新的 generation，旧关闭请求、延迟显示任务不能影响新窗口。窗口销毁会移除原生点击监视及回调；纳入两分钟空闲修剪的窗口计数。
- 当前 Slint 1.18 的公开可访问角色不含 menu/menu-item，使用可访问列表和条目，提供本地化名称、选中状态和默认操作。
- 不再使用原生 HMENU 自绘、专用 GDI 字体/画刷或 WM_DRAWITEM/WM_MENUCHAR 处理。

## 验证（2026-09-27）

- `cargo build --workspace`、`cargo test --workspace`：231 项通过，15 项现有环境相关测试忽略。
- `cargo clippy --workspace --all-targets -- -D warnings`。
- `cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- `cargo build --release -p lexift-app`，`cargo fmt --all -- --check`，`git diff --check`。
- 新增：负坐标解码、首字母/Unicode 匹配、动作映射、窗口释放与过期任务、菜单主题跟随测试；将托盘菜单纳入 12 种语言的渲染测试入口。
- Windows 1920×1080、125% DPI 实测：真实托盘溢出面板右键；浅/深色、中/英文与悬停；Shift+F10、菜单键、Escape 返回托盘焦点；首字母打开设置；鼠标打开主窗口；End+Space 退出。
- 连续 12 次打开/外部点击关闭，展开时窗口数为 1、关闭后为 0；已展开时再次请求仍只有一个菜单。
- 四角锚点通过通知区消息注入验证，菜单窗口均位于工作区内。负坐标和不同物理尺寸由已有 placement 测试覆盖。
- 实图与构建日志保存在本机 `target/tray-menu-review/`（不纳入版本管理）；临时主题/语言设置已恢复。

本次环境只有一块显示器，未进行不同 DPI 双屏实测；未重启用户的 Explorer，图标恢复路径保持原有实现。

## 任务栏图标修复（2026-09-27）

- 菜单专用准备回调由 App 注入，Platform 同时设置 Winit `skip_taskbar` 和原生 `WS_EX_TOOLWINDOW`，清除 `WS_EX_APPWINDOW`。显示后再次确认样式，防止 Winit 的显示流程恢复缓存样式；普通窗口不使用该回调。
- Winit 使用现有 0.30.13 依赖，仅在 Windows 的 Platform 构建中直接引用，避免改变其他平台的构建依赖。
- Release 实测鼠标右键、Shift+F10 唤出、重复打开、Escape 和外部点击关闭；菜单原生扩展样式为 `0x198`（TOOLWINDOW 存在、APPWINDOW 不存在），任务栏没有菜单图标，任务切换列表中没有菜单。
- 主窗口及 Settings 仍正常显示任务栏按钮；结束检查后恢复后台运行。
- Platform/UI 测试、专用样式测试、workspace 构建、全 feature Clippy 与 Release 构建通过。实图：`target/tray-menu-review/taskbar-fixed-release.png`、`taskbar-fixed-alttab.png`、`taskbar-main-normal.png`、`taskbar-settings-normal.png`。
