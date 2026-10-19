# 截图启动与按点区域识别验证

日期：2026-10-19。接续 [上一轮启动优化](screenshot-startup-performance.md)，在 `codex/screenshot-point-query` 分支验证；本轮没有暂存、提交或推送。证据与合成夹具位于忽略的 `target/screenshot-startup/`。

## 当前实现

- 首帧只等待冻结像素、稳定的顶层窗口及原标注合成，删除抓图前 80ms 元素遍历和后置 250ms 整树校验。内部元素不再承诺与底图严格冻结一致；取色、编辑与输出仍使用原冻结像素。
- Core 隔离会话、查询编号及取消；App 调度现有 ScreenCapturePort 的非阻塞查询。Windows 唯一 MTA 工作者从冻结 HWND 根节点查询包含物理鼠标点的分支，缓存几何、离屏与运行时身份，最多 128 节点、16 层、80ms，连接及事务超时各 50ms。只保留最新待处理请求，慢窗口在会话内冷却 1 秒；不读文字或传递 COM 对象。
- 窗口 PID、物理范围和所属会话须仍一致；失败使用整窗。已确认叶子缓存最多 150ms，同一叶子刷新保留滚轮层级，离开后重置。按下消费最后实际绘制完成的范围，冻结本次手势，迟到结果不能追溯修改点击。
- 跨屏消息可乱序，旧宿主的 Exited 只清理仍落在自身物理范围的查询；不得取消新宿主已经提交的请求。现有跨屏取色回归同时验证查询身份不会被旧退出消息清除。
- Bootstrap RGBA 转换及遮罩预处理移到后台。全部宿主隐藏准备完成后显示；同一会话按屏幕组执行 bootstrap 和完整反馈两次桌面同步，每阶段一次 DwmFlush，保留实例检查、无边框保护、激活及 C 复制。
- 普通 OCR 入口、编辑功能、图像输出和渲染器选择保持现有路径。不新增依赖、配置、crate、Port 或界面文案。

## 程序与测量口径

最终 Release `target/release/lexift.exe` 的 SHA256：

`835DE928D2363C7120F06529CE34B46BAF7546544962CB9E2DB2C457E396E727`

对照副本 `target/screenshot-startup/lexift-before-point-query.exe` 为上一轮 B4D99F2E… Release，不使用更早的 730C56B2… 副本代替本轮基线。进程路径及摘要见 `release-process.json`。

本轮机器新增第三屏，当前物理桌面为：125% `2560×1440 @(0,0)`、100% `1920×1080 @(2560,0)`、150% `2560×1600 @(4480,0)`。不能直接与上一轮双屏数字比较。

- 每版各 20 次进程首次截图、20 次驻留重复截图。Lexift 从现有截图按钮鼠标事件开始，PixPin 从 Ctrl+1 开始；进程启动、夹具准备和脚本稳定等待不计入。
- 端点要求全部屏幕实际底图探针就绪、交互反馈探针区别于纯底图或蒙层、所属窗口为前台且接收输入。Lexift 同时核对三个宿主的物理范围，不能只看 HWND 可见或日志交接。
- 自动化探针是桌面观察口径，不是光电测量。P95 为排序第 19/20 项，中位数为中间两项平均。各组顺序运行，期间没有编译负载。
- 修正了测试脚本自身的 DPI 虚拟化：Forms 初始化后设置进程 DPI 并不可靠，改为明确设置执行线程的 PMv2 上下文，再读取原生物理范围。旧试跑和仅两个宿主的数字不进入本轮正式统计。
- 正式统计使用一次整帧桌面快照读取全部探针，不逐点调用桌面 GetPixel；后者仅读取 12 个像素就实测花费约 86.92ms，且探针并非同一时刻。每次三者均先把同一夹具置前，再触发截图。下表包含桌面读回及轮询的观察开销，是本机同口径观察值，不能把几毫秒差距都归因于产品本身。

| 版本 | 首次中位数 | 首次 P95 | 重复中位数 | 重复 P95 |
| --- | ---: | ---: | ---: | ---: |
| 修改前 Release | 449.66ms | 468.62ms | 447.00ms | 477.50ms |
| 本轮 Release | 316.21ms | 329.29ms | 309.76ms | 321.92ms |
| PixPin 3.5.5.1 | 293.70ms | 307.99ms | 301.98ms | 320.41ms |

首次 P95 下降约 29.7%，重复 P95 下降约 32.6%。**首次 ≤350ms 达标，重复 ≤250ms 未达标，超出 71.92ms。** 原始数据为 `frame-before-benchmark.jsonl`、`frame-release-benchmark.jsonl`、`frame-pixpin-benchmark.jsonl`，均为 40 行；汇总见 `frame-performance-summary.json`。`point-physical-*` 属于此前逐点读回及中间产物试跑，保留溯源，不替代此处正式数据。

### 剩余阶段

另做 5 次带阶段日志的诊断，不混入上述 20＋20 次正式数据。下列项目存在包含和重叠关系，不能相加；日志为 `frame-stages.log`，逐次值和汇总为 `frame-stage-breakdown.json`、`frame-stage-summary.json`。

| 阶段 | 中位数 | 最大值 |
| --- | ---: | ---: |
| 顶层窗口首次枚举 | 5ms | 6ms |
| 冻结底图就绪，含排除确认、桌面同步、采集与复制 | 91ms | 100ms |
| 后台 bootstrap 转换及遮罩 | 15ms | 17ms |
| 准备开始至首帧派发，包含前两项 | 113ms | 124ms |
| 三屏 Slint 像素复制合计 | 4.99ms | 5.59ms |
| 三屏宿主构造，包含像素复制 | 5ms | 5ms |
| 原生隐藏准备及显示尝试 | 42ms | 43ms |
| 三屏首个软件客户区绘制 CPU 合计 | 17.99ms | 22.13ms |
| bootstrap 阶段组 DwmFlush | 19.52ms | 21.19ms |
| 完整反馈阶段组 DwmFlush | 5.20ms | 8.63ms |

五次均使用快路径，每次两个组同步、每组 3 个宿主。输入到组反馈完成通知中位数 278.99ms、最大 289.24ms，**这是内部诊断，不能替代桌面像素验收端点**。本组外部观察器末次桌面读回中位数 56.14ms、最大 80.70ms，可能与程序呈现重叠，不能直接从正式结果中减去。剩余时间主要在底图采集、多屏原生准备、软件绘制及桌面呈现／事件排队；内部元素遍历已经不在首帧关键路径。

### 立即进入编辑

使用包含三屏的同一普通窗口夹具，在完整反馈可操作后立即点击选区，分别测 20 次（每版 1 次进程首次、19 次驻留）。终点为旧框选宿主全部退休、三个编辑画布及工具条就绪；退休沿用现有编辑首帧交接，另保存桌面截图。沿用同一逐点启动探针的独立对照，只比较本表的松手至编辑阶段，不混入整帧首帧表。

| 版本 | 进入编辑中位数 | P95 |
| --- | ---: | ---: |
| 修改前 Release | 425.63ms | 437.73ms |
| 最终 Release | 427.52ms | 432.21ms |

中位数增加约 0.44%，P95 下降，未超过 10% 退化限制。证据为 `point-edit-before-final-edit.jsonl`、`point-edit-final835-edit.jsonl` 及对应逐次桌面图；最终汇总 `point-edit-final835-summary.json`。

## 自动化回归

- Core 全套 171 项通过；删除了只验证旧双轮快照的 1 项过时测试。UI 全套 451 项通过、11 项忽略，包含框选路由 13 项。
- App 截图 6 项、标注导出 7 项通过。验证像素准备不查询可访问性、快路径失败回退、请求取消及旧结果隔离。
- Platform 屏幕采集相关 20 项通过、9 项环境相关用例忽略；另跑原生框选宿主 4 项，包括两阶段分组屏障、实例及窗口销毁、缓存取消、无边框样式重写与激活消息。工作者合并/协作取消用例通过。
- Core、App、UI、Platform all-targets Clippy `-D warnings`、格式及 diff 空白检查通过；Slint 随 UI 编译通过，Windows Release 构建成功。最终清理后的日志为 `point-cleanup-core.log`、`point-cleanup-ui-final.log`、`point-cleanup-app-final.log`、`point-cleanup-export.log`、`point-cleanup-platform.log`、`point-cleanup-clippy-final.log`、`point-cleanup-release.log`；分组屏障与工作者日志为 `point-group-tests.log`、`point-worker-tests.log`。
- 最终跨屏退出乱序修改重跑 UI 框选 13 项；能力工厂移出会话锁后重跑 App 截图 6 项和 Clippy，均通过。对应最终日志 `point-exit-ui.log`、`point-owner-app.log`、`point-owner-clippy.log`、`point-owner-release.log`。

## 原生交互证据

最终 Release 在 125% / 100% 屏幕完成 9 个控件用例：按钮、父区域、整窗、输入框、按钮、列表、父区域、跨屏父区域及识别后转自由框选。9/9 采用预期尺寸，实际图像输出和冻结 C 取色像素通过；截图、输出及窗口身份保存在 `button-125-*`、`edit-100-*`、`cross-parent-*` 等文件，执行日志 `point-controls-final.log`。测试前需把原标注工具条移离取样点，因为截图会排除工具条，不能把工具条白色像素当作底图期望值。

同组修改前版本在 8 项识别目标中精确命中 3 项，其余有效回退到父区域或整窗；本轮为 8/8。自由框选两版均通过，不纳入元素识别分母。比较文件为 `point-coverage-before.jsonl`、`point-coverage-final.jsonl`，进程摘要为 `point-controls-baseline-process.json`、`point-controls-final-process.json`。这只是合成夹具覆盖率，不推导所有程序的比例。

置前夹具后，跨屏返回控件的 125% / 100% 各 20 次均呈现精确边框，观察中位数分别 60.03/59.60ms，P95 为 71.71/62.26ms；记录为 `point-recognition-final.jsonl`、`point-recognition-summary.json`。未置前的多窗口夹具首屏曾回退整窗，保留 `point-recognition-observed.jsonl` 和相应失败帧，不把这个差异归结成已证明的提供者原因，也不声称识别无条件全覆盖。产品没有为了识别而激活目标窗口。

Edge 本地夹具按钮 `200×50`、父区域 `681×431` 均精确命中，实际输出及冻结色值 `#F0D080` 通过。该进程使用专用测试 profile、`--disable-gpu --force-renderer-accessibility`，不能把结果推广为默认设置或所有网页。证据为 `point-browser-final.log`、`browser-button-*`、`browser-parent-*`。源桌面预检容许 1 个色阶误差，冻结颜色及导出像素仍按夹具原色精确核对。

三屏顶部共检查 385680 个像素，无白条或标题重绘，见 `point-whitebar-final.jsonl`、`point-whitebar-final-*.png`、`whitebar-process.json`。125% / 100% 的半透明夹具及截图前原标注分别核对通过；源桌面与重合成原标注允许 1 个色阶的栅格差异，改变外部夹具后冻结 C 取色须逐值不变，取消后外部变化可见。记录为 `point-layer-results.jsonl`、`point-layer-frozen-*`。透明夹具在计时器中明确显示并置前，避免把隐藏窗口当作透明采集成功。

## 证据边界

UIA 超时预算为协作停止规则，不承诺有问题的外部提供者一定及时返回；唯一工作者和最新请求合并避免增加线程或阻塞 UI。自动化模拟的阻塞提供者不能替代所有第三方程序的兼容性验证。

本机为 Slint 软件渲染、当前 Windows 浅色主题。未进行 GPU 渲染器原生验收、负坐标显示器的实机布置、所有第三方程序及真实长期阻塞 UIA 提供者测试；负坐标与旧结果等由纯函数／UI 自动化覆盖。150% 第三屏覆盖首帧和顶部像素，未逐项重做其全部编辑操作。没有用高速相机或独立呈现追踪消除桌面读回开销，也没有全程逐帧录像，不把截图或内部回调当作这些范围均已覆盖。

PixPin 的静态证据、运行观察与未知部分继续分开，见 [DLL 分析](../planning/pixpin-startup-ida-analysis.md)。本轮实现没有引入 MSAA，也不声称 UIA 一定优于 PixPin 的策略。
