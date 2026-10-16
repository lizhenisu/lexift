# 截图选区键盘精确调整验证

日期：2026-10-16。范围仅截图编辑的“鼠标操作”，不增加全局快捷键或标注撤销步骤。

## 实现

- Core `RegionEditor` 保存键盘调整前快照，物理像素移动与固定左上角调整尺寸；重复按下复用快照，松手提交，取消恢复。移动与鼠标复用边界平移限制，键盘与鼠标事务互斥。
- UI 画布、工具条的 Slint FocusScope 转发未消费方向键；Winit 观察器只提供系统重复及焦点通知，不消费原生按键、不注册钩子。窗口身份、会话及焦点版本排除迟到事件。
- Escape 回滚并抑制仍按住的方向键；失焦、切工具、销毁取消未提交事务。键盘期间工具条不隐藏、不随每次重复移动；提交／取消后定位。选区事件在 Registry 借用释放后发送，输出先提交同步再冻结。
- 首次框选是用户主动操作，画布准备后校验前台身份并尝试一次激活；失败不循环。工具条准备完成的回调增加窗口版本校验。

## 自动化

- `cargo test -p lexift-core region_selection`：5 项通过，覆盖负坐标、1／10px、四方向、宽高增减、最小值、边界、混合、提交／取消和鼠标互斥。
- `cargo test -p lexift-core screenshot`：4 项通过，覆盖原有会话和输出状态。
- `cargo test -p lexift-ui --lib annotation::`：最终 306 项通过、11 项原有显式忽略。
- `cargo test -p lexift-ui --lib screenshot::keyboard`：5 项通过（包含在上述 306 项中）。覆盖系统重复、多键及修饰键、Escape 抑制、无变化无事件、焦点版本、旧输入、工具／贴图／输出／参数输入拒绝、真实 Slint 画布与工具条路由、一次激活、输出顺序及撤销隔离。
- `cargo clippy -p lexift-ui --all-targets -- -D warnings`、`cargo fmt --all -- --check`、`git diff --check` 通过。Slint 两个组件经过 UI 测试及 Release 编译。

日志、脚本与桌面证据均在忽略目录 `target/screenshot-keyboard/`，编译检查日志为 `target/screenshot-key-*.log`。软件窗口测试只验证路由与状态，不替代 Windows 原生焦点结论。

## Release 与原生验证

`cargo build -p lexift-app --release` 最终构建通过（3m57s）。实测进程 PID 5780，路径 `D:\MyDocuments\Projects\lexift\target\release\lexift.exe`；文件时间 2026-10-16 16:39:24，SHA-256 `4FBB5A2A08E58861E45162162B7AAE4AE446C22B4FF3EE8E810A9A8CAAB7F315`。构建完成后重新启动进行以下测试，未沿用前一轮二进制。

实际桌面为左侧 2560×1440／125%、右侧 1920×1080／100%，原点均非负。`native.ps1` 校验项目路径、操作所属 HWND，并记录前台与工具条存在状态；全部图片保留在忽略目录。

| 场景 | 实测结果与证据 |
| --- | --- |
| 首次松手后立即按键，100%／125% | 两屏首次框选完成均为所属画布前台，无需再点击。`96/events.jsonl`、`120/events.jsonl` 最后一组 `before` 记录 |
| 320×240 选区，右 1px、Shift 下 10px、Ctrl 右扩 1px、Ctrl＋Shift 下扩 10px | 两屏复制均为 321×250；工具条左移测试再回到原 x，y 为原值＋10。`output-micro.png`、`after-micro.png`、`toolbar-left.png` |
| 标签／边框／输出 | 两种 DPI 各比较 300×210 的冻结源内部重叠区域，与输出逐像素一致。排除 125% 控制点范围后无差异，`pixel-check.json`、`check.py` |
| 重复按住与 Escape | 移动后按 Escape，再发送仍按住方向的重复消息并松开；两屏 `after-micro.png` 与 `after-escape.png` 完整画面一致 |
| 画布和工具条焦点 | 原生前台从画布切至所属工具条后，左箭头有效；全过程只有一条可见工具条。记录含实际 HWND |
| 失去焦点后取消 | 移动中切到同进程主窗口，回画布后旧重复不再调整；`before-focus.png` 与 `lost-focus.png`、`after-focus.png` 完全一致，`during-focus.png` 确有变化。`focus-check.json` |
| 100% → 125% 跨屏 | 从右屏 x=2680 起，以 Shift＋左重复 200 次移到左屏 x=680；尺寸保持 320×240，标签及工具条转入左屏，复制也是 320×240。`96/cross-primary.png`、`96/output-cross.png` |
| 最小尺寸与边界 | 125% 屏幕将宽高缩至 1，持续缩小不会翻转，再向左／上移至桌面边界并继续输入。复制为 1×1，像素为冻结底图 `(150,188,199,255)`，无边框或标签。`120/edge-1px.png`、`120/output-minimum.png` |

测试脚本初次未对方向键设置 Windows 扩展键标志，使注入的 Shift 组合被当作数字小键盘输入；已修正后重新执行最终 Release 组合键与像素验证。此测试工具问题没有修改应用的修饰键逻辑。

完成后退出测试截图与空标注，项目最终 Release 保留后台运行。

## 未覆盖

- 本机没有负坐标显示器排列，负坐标仅由 Core 回归覆盖，未移动用户系统显示器布局。
- 多方向同时按下、按住期间更换修饰键、工具切换及过期回调由自动化覆盖，未穷尽所有原生按键交错；菜单、文字／水印及保存对话框焦点组合没有逐项做原生实测。
- 本轮原生输出检查采用系统图片剪贴板；未再逐项重复 PNG 保存、贴图和全部标注工具的人工测试，相关现有自动化随 306 项回归执行。
- 11 项既有忽略测试以性能／实验夹具为主，本次未强制开启。未做长时间按住压力测试。
- 手动宽高输入、比例锁定属于后续任务；本轮不实现。
