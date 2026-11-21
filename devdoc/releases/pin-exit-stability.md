# Windows 贴图退出稳定性验证

实施分支为 `codex/pin-exit-stability`，起点为 `main 0dec026`。保留[贴图管理记录](pin-manager.md)中的首次失败证据。

## 基线与归因

- 使用 Windows MSVC Release 与默认渲染器，通过 Explorer 的真实托盘菜单退出。引入贴图管理前的 `f2c1a88` 在空载、仅图片、仅文字、混合贴图和实时贴图场景都复现了 `0xC0000409`。`0dec026` 在空载和打开或关闭过管理窗口的场景也复现。脚本把 Shell 控件未出现与实际退出异常分开保留。
- 原 Release 捕获异常 `0xC0000409`，子码为 `7`，调用栈经过 Rust 线程局部清理和 `LdrShutdownProcess`。同一故障的带符号 Debug 堆栈显示，UI 线程局部注册表晚于 `Lexift stopped` 才释放 `WindowLifecycleCallbacks`，分别触发托盘监听和标注渲染线程的 `JoinHandle::join`。此时 Rust 的线程退出结果检查发生 panic。
- 托盘和标注渲染的引用随 UI 注册表在所有线程退出前显式释放。App 在首次退出请求后忽略重复请求和迟到事件，翻译快捷键回调以弱引用访问控制器。

## 验证口径

- 退出用例记录操作时的窗口及 DPI、退出码、UI 和 App 阶段、托盘及渲染线程的 join 结果。实时贴图同时核对采集 worker 退役。托盘由 Explorer 所在主屏唤起，贴图及管理窗按矩阵移到对应屏幕。
- 原始对照矩阵有 21 次实际退出返回 `0xC0000409`。另外三次在脚本准备阶段失败，补测均复现相同退出码。首轮修复版中，125% 屏有测试贴图挡住托盘图标，受影响的尝试在发出退出请求前终止。调整脚本停放位置后重跑该屏，保留首轮记录。
- Slint 软件渲染器的缓冲区断言在此前 Debug 和本轮一次 Release 扩展交互中发生，均在退出请求前。原证据见上一轮记录及本轮 `release-interactions.log`。该问题单列。
- Core、App、UI 和 Platform 共 1,282 项回归通过。新增用例验证事件循环报错时也释放桥接回调、重复退出请求只调度一次，以及退出后丢弃迟到事件。严格 Clippy、格式、Slint 编译和 Windows Release 构建通过。
- 脚本、构建哈希、异常调用栈和逐轮结果保存在忽略的 `target/pin-exit-stability/`。

## 最终 Release

| 显示器缩放 | 七种场景，各三轮 | 混合贴图连续复验 | 自然退出码 |
| --- | ---: | ---: | --- |
| 100% | 21/21 | 10/10 | 全部为 `0` |
| 125% | 21/21 | 10/10 | 全部为 `0` |

- 62 轮均通过真实托盘入口退出，未使用强制清理。各轮进程退出后没有所属窗口或托盘窗口，日志确认托盘监听线程和标注渲染线程均完成 join。六轮实时贴图退出均记录采集 worker 完成退役。脚本逐轮重新启动进程并验证真实托盘登记。
- 两屏另各通过一轮交互回归。验证单张关闭、从托盘恢复上次关闭的图片、关闭管理窗口后继续使用贴图，以及 Alt+S 退役管理窗和定位边框、框选接焦、Escape 后恢复贴图。两轮最后均自然退出并返回 `0`。
- 100% 结果取自 `fixed-matrix.jsonl` 中 DPI 为 96 的记录，125% 取自 `fixed-parked-matrix.jsonl`。交互证据在 `interactions-fixed-96/` 和 `interactions-fixed-120/`。

| 构建 | SHA256 |
| --- | --- |
| `f2c1a88` Release | `94ab9c6e48c24c7fd04a43b9845ddf416df8f8d224ce14d30ca913f3fa785343` |
| `0dec026` Release | `60748e26329a445a6f53e3d1cf2b9a42ddd668a2a62d7cdf1ef7cad823f7f6a9` |
| 修复版 Release | `63d6be173075d7cabf32b0d904a50a69857ce94c2199d6b55d67ad344d88ac5e` |
