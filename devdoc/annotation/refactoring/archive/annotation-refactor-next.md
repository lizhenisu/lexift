# 标注模块下一轮重构计划

> 历史计划与交接记录：后续执行顺序统一见[标注重构执行计划](../annotation-refactor-plan.md)。下文的“下一轮”“当前交接”保留当时语境，不代表今天仍需重复实施；实现及实验结果见[详细记录](../annotation-refactor.md)。

## 范围与原则

本计划针对标注 UI、对象编辑及实时效果链路。以当前源码为起点，已完成的参数来源分离、实例校验和菜单构建迁移不重新实施。历史结果见 [实现与实验记录](../annotation-refactor.md)；旧日志不能证明当前工作区通过。

目标是减少状态耦合、重复工作和用户可见延迟。文件行数不是验收指标。保持七个 crate 和依赖方向，不新增配置或依赖，不暂存、提交或推送。每批只改变一个主要行为边界。

## 当前依据

- `annotation.rs` 的 Registry 仍同时协调窗口、文档、编辑、手势、参数和渲染；已有子模块，但部分回调仍依赖整个 Registry。
- `annotation_choice.rs` 的窗口构建已有明确输入，菜单关闭、所属窗口和回调协调仍直接访问 Registry。
- 场景副本迁移已完成一轮：原生与软件绘制共用 Document 借用视图，后台以 Arc 共享最新不可变快照。剩余成本是完整内容比较和手势预览几何更新，后续须先测量再优化，不重复实施已完成迁移。
- 实时效果已有 GPU 路径和呈现来源诊断，但实际显示间隔、输入延迟、画面正确性及故障恢复尚未完整验收。
- `sample_gpu_latest` 实验已因显示指标未稳定改善而撤回；当前保留采集变化检测。新增刷新周期诊断用于后续调度归因，不能把单项运算降耗等同于体验改善。

## 执行批次

### A：固定可恢复基线

1. 核对未收尾实验的日志、源码和性能口径。未改善整体显示指标的实验仅撤回其自身改动，保留其他工作区修改。
2. 对当前源码运行相关回归、workspace 测试、Clippy 和 Windows Release；记录源码状态、环境及失败项。
3. 固定行为清单：二级工具条保持展开、参数操作后继续输入、旧回调失效、取消不留对象、一次操作一次撤销、铅笔不可编辑。

出口：可重复的当前基线，明确哪些结果来自夹具、哪些来自真实应用。无关失败记录后单独处理。

### B：完成菜单、焦点与窗口所有权

1. 菜单接收展示请求、参数快照与带目标身份的选择事件，减少对 Registry 的直接访问。
2. 普通菜单、水印时间菜单和选色器采用相同实例校验及关闭规则；二级工具条仍独立管理。
3. 延迟显示、参数操作结束、焦点恢复由所属窗口或编辑实例拥有并取消，销毁时释放组件与回调。
4. 数字工具/字段协议限制在 Slint 边界，内部使用明确类型；不一次重写全部事件系统。

出口：同一插入位置继续输入；三级关闭不收起二级；快速关闭重开后旧任务无效果；输入控件和弹层操作期间不被抢焦点。

### C：统一手势和撤销收尾

1. 梳理创建、拖动、缩放、连续参数修改的开始、更新、提交、取消路径，优先消除遗漏和重复提交。
2. Core 保有文档事务与撤销，UI 保有输入捕获与临时手势；沿用现有接口，确有缺口时先单独评审。
3. Escape、捕获丢失、工具切换和会话关闭进入相同取消规则；无变化操作不压入历史。

出口：创建与拖动一次撤销；取消恢复编号、对象及选择；已有对象编辑行为一致。

### D：验证增量失效与剩余热路径成本

1. 已完成：编辑投影只拥有活动编辑对象，其他对象通过 Document 借用；软件任务复用 Arc 快照。保留必要跨线程不可变所有权。
2. 测量长笔刷预览更新、完整内容比较、签名计算、栅格范围及上传量，只优化有实际耗时证据的路径；审计屏外内容与元数据变化的失效完整性。
3. 文档内容、效果依赖和交互覆盖层分开失效；光标闪烁不重新采集或处理模糊。
4. 删除对象、移出视口和结束会话时清理所属缓存；避免引入无明确收益的通用缓存框架。

出口：静止状态无重复效果计算；局部修改只重算受影响内容；长轨迹复制及内存增长有前后证据；文字画面不变。

### E：实时链路性能与恢复

1. 分别测采集、等待、GPU 运算、输出比较、提交及系统呈现，结合 QPC 与刷新周期解释间隔抖动。
2. 校验源像素版本到冻结候选及呈现结果的对应，避免用总提交次数替代有效显示更新。
3. 每次只改变一个调度或算法变量；保留画质、静止去重和资源生命周期回归，失败实验不进入默认路径。
4. 验证设备失效、重建失败、首次采集失败与软件降级，保留最近成功画面并释放旧资源。

出口：Release、同机变化源的三组场景达到约定指标：有效更新至少 57FPS，间隔 P95 ≤20ms，GPU P95 ≤12ms，输入到显示 P95 ≤50ms。刷新率限制必须解释并单独确认，不能静默降低门槛。马赛克每通道误差 ≤1，模糊有效 σ 偏差 ≤10%，无接缝。

## 每批交付与最终验收

- 每批提供改动边界、针对性回归、当前构建结果、已知限制及可回退范围；不为拆文件而扩大变更。
- 涉及 Slint 的批次执行编译检查；影响公共链路的批次执行 workspace 测试、Clippy `-D warnings` 和 Windows Release。
- 最终在真实应用覆盖中文输入、选区与焦点、所有工具、撤销、关闭重开、100%/125% DPI、负坐标及跨屏；保存画面和资源释放证据。
- 原生性能夹具与组件回调测试不能代替真实应用验收。缺少设备或交互证据时明确保留未验收项。

建议从 A 开始，之后 B、C、D、E 顺序推进。性能诊断可穿插采样，但不与焦点或文字排版迁移混在同一批。

## 本轮起点与优先级（2026-10-03）

最近完整门禁对应软件 Document 迁移：workspace 447 通过、39 忽略，含 Slint 编译；Clippy 与 Windows Release 通过。证据见 target/annotation-software-document-{workspace,clippy,release}.log。这些结果不覆盖随后实验。

交换链未就绪时提前准备效果纹理的实验已经测量并撤回：高强度模糊的系统显示版本约 59.403FPS、QPC 间隔 P95 27.784ms，未改善整体显示门槛。GPU P95 2.837ms、取帧到提交 P95 8.792ms 是该夹具的中间指标，不能据此认定体验改善。测量见 target/annotation-effects-before-swap-large-blur.log；撤回后原生单次源变化及拒绝提交回归通过。

| 优先级 | 工作 | 交付边界 |
|---|---|---|
| P0 | A：收尾实验与固定基线 | 记录性能前后结果及当前门禁，失败实验撤回 |
| P1 | B：窗口、弹层与焦点所有权 | 迁移一个窗口种类后验证，再推进下一个；Registry 保留协调职责 |
| P1 | C：手势与撤销收尾 | 创建、变换、参数连续调整逐类迁移，共用取消规则 |
| P2 | D：增量失效与热路径 | 以耗时、复制量和画面一致性证明收益 |
| P2 | E：实时显示与故障恢复 | 源像素、候选、实际呈现证据及设备恢复验证 |

每批开始列出涉及文件和必须保持的行为；结束记录针对性回归、影响范围匹配的门禁和剩余风险。若改 Slint，使用既有设计值与组件并执行编译检查。不会把文件变短或测试数量增长当作收益。

性能限制单独处理：受控夹具曾测得有效更新约 59FPS、显示间隔 P95 约 27.8ms；GPU 运算达标不代表显示间隔达标。推算刷新周期约 6.95ms 尚不是系统模式查询，须验证刷新率与调度贡献，继续保留原验收门槛。真实应用、输入到显示、100%/125% DPI、中文输入法和资源释放证据仍未完整取得。


菜单返回任务已迁入菜单 State 的可取消定时器，替代全局 next_frame；关闭、替换及销毁会停止任务，仍保留会话、实例、目标校验和 16ms 的 Escape 路由保护。B 批继续审计其余菜单协调及编辑事件任务。当前完整门禁：workspace 447 通过/39 忽略，Clippy -D warnings 与 Windows Release 通过（target/annotation-menu-transition-{workspace,clippy,release}.log）。


文字/序号说明的内容、光标与完成回调也已迁入编辑实例 InputWork：同轮更新合并并读取最新输入状态，完成/取消停止待更新，退休后保留的控件不能继续排队。当前门禁更新为 workspace 448 通过/39 忽略，Clippy 与 Windows Release 通过（target/annotation-editor-input-{workspace,clippy,release}.log）；真实 IME/DPI 验收仍保留。


活动手势取消已共用 cancel_active_gesture，修复矩形/铅笔绘制中切换到箭头仍可被旧松键提交的问题。覆盖模式切换、Escape、捕获丢失及对象变换回滚；仍须继续审计提交与连续参数事务。当前门禁 workspace 449 通过/39 忽略，Clippy、Windows Release 通过（target/annotation-gesture-retirement-{workspace,clippy,release}.log）。


真实 WGC 拥有源与冻结提交候选已新增独立像素采样核验：四个色带画面、两种效果、两次拒绝提交期间真实源推进，24 点最大通道误差 0。原生夹具显式通过，最终 workspace 449 通过/40 忽略，Clippy 和 Windows Release 通过。该证据仅覆盖指定采样点到 native commit，不替代全帧、物理显示或输入延迟验收（target/annotation-owned-pixels-native-final.log）。


GPU 有界重建现在独立保留最后成功呈现的 Surface，替代提交、会话更换和关闭时释放；worker 的独立 COM apartment 覆盖保留资源。原生故障注入验证等待时桌面上一帧不消失及重建中 clear 无残留，WGC 像素对应回归通过。当前门禁 workspace 449 通过/40 忽略、Clippy/Windows Release 通过。接下来仍需处理 UI Presenter.fail 到软件首帧的交接，不能以重建阶段保留等同完整降级无空档。


GPU 到软件交接已延迟原生 clear，直到当前完整效果与所有画布提交成功；软件任务身份同时包含完整不可变快照。当前 workspace 451 通过/40 忽略，Clippy 与 Windows Release 通过，worker 部分帧标记回归通过。尚未取得真实应用交接画面，不宣称跨屏原子呈现或物理连续性。下一批可继续迁移会话级排队渲染与首个放大镜延迟初始化的任务所有权，避免全局 single_shot 仅依赖旧 generation/revision 校验；同时保留真实交接验证项。


画布排队渲染与首个放大镜初始化已迁入会话拥有的 frame::Tasks；关闭、替换和销毁停止任务，保留 16ms 时序与身份校验。当前 workspace 452 通过/40 忽略、Clippy/Windows Release 通过（target/annotation-frame-tasks-*-final.log）。后续继续处理菜单事件与连续参数收尾的 ZERO 排队所有权及 Registry 协调边界，真实应用交接与性能证据仍缺失。


菜单 Slint 输入、初次显示和外部关闭转入每实例 FIFO InputWork；take/Drop 取消队列，保留旧控件也不能重排，连续参数的两轮 ZERO 收尾同样由实例持有。实际回调及连续改色一次撤销回归通过。当前 workspace 453 通过/40 忽略，Clippy 与 Windows Release 通过（target/annotation-menu-input-*-final.log）。下一步继续减少菜单执行对 Registry 的协调依赖，审计工具条/水印输入任务所有权；性能、真实应用与完整目标尚未验收。


菜单完成方案已从 Registry 判断拆出：SelectionEffect、CloseAction 与 ReturnDestination 明确参数/时间预设及关闭目标；延迟返回统一实例与 owner 校验，父请求在借用外打开。当前 workspace 454 通过/40 忽略，Clippy 与 Windows Release 通过（target/annotation-menu-completion-*-final.log）。后续继续迁移工具条与水印编辑窗口事件任务，梳理菜单所属窗口身份；完整性能及真实应用验收尚未完成。


工具条、参数面板、水印编辑与菜单共用 annotation_window::InputWork，Slot open/clear/Drop 管理每实例 FIFO 队列。当前 workspace 455 通过/40 忽略，UI 204 通过/1 忽略，Clippy 与 Windows Release 通过（target/annotation-window-input-*-final.log）。保留组件、待输入取消、无循环所有权及旧工具条重排回归通过。下一批审计画布输入与剩余会话级 ZERO 渲染转发，避免它们只靠 generation 检查；性能、真实应用和完整验收尚未完成。


画布快捷键与静止光标任务已由每屏 Layer InputWork 持有，参数/正文更新直接排队会话渲染；annotation*.rs 的全局 single_shot/later 已清除。修复无身份自动面板任务，绑定当前会话、画布、选择和工具组。当前 workspace 456 通过/40 忽略，Clippy 与 Windows Release 通过（target/annotation-canvas-input-*-final.log）。后续进入手势提交与参数事务收尾审计，保留菜单 owner 协调、热路径测量及真实应用/呈现/性能验证项。


活动文档手势期间的合法参数现在先等待，完成/取消后在参数重新投影前应用；同一目标合并最新值，菜单字段更新保留已等待的其他字段。最终 workspace 458 通过/40 忽略，Clippy -D warnings 已通过，Release 仍待串行门禁收尾（target/annotation-deferred-parameters-*-final.log）。下一批重点：update_text_handle 从 active:false 越过阈值时直接 begin_drag，须先处理此前参数事务，并审计 gesture_object 是否覆盖阈值前已应用的参数；补充序号默认目标、文字角点和面板退休的等待队列回归，以及马赛克 pending 分支的重复 cancel_drag。真实应用、性能与完整验收仍未完成。


文字控制点入口进一步确认：阈值前参数可正常应用，但 update_text_handle 激活仍使用按下时的 gesture_object，可能覆盖后来颜色/字体/描边。下一批首次激活应读取当前对象或编辑草稿，并在非编辑文档 begin_drag 前完成已有参数事务；active:true 的编辑草稿也需要阻止参数与几何副本互相覆盖，区别于 owns_document_transaction。finish_text_handle 应在尺寸重新投影前应用等待值。上一批 Release 串行进程 session 20628 仍存活，rustc PID 6264 CPU 时间持续增加，未重启或结束；源码尚未启动下一批修改。


文字控制点激活修复已实现：首次激活读取最新对象/草稿并结束此前参数事务；active 编辑草稿通过 blocks_parameter_edits 等待参数而不拥有 Core 几何事务。完成前先应用等待值，再同步字号；完成/取消及独立撤销回归通过。当前最终 workspace（含 doc-tests）461 通过/40 忽略（UI 210/1），Clippy -D warnings 通过，Release 仍由 session 1631 收尾；上一批 Release session 20628 仍活跃。下一批先完成当前门禁，再补序号默认目标及面板退休队列回归；Escape 对 pending 马赛克额外 cancel_drag 与专用协调器的取消重复，继续审计。当前源码没有性能或真实应用验收证据。


Escape 与专用马赛克协调器的重复取消已修复：仅统一手势入口回滚其拥有的几何，接受参数先完成，再丢弃 provisional 历史。无手势 Escape、活动 pending 移动的协调器取消/Escape 与无关全局不透明度撤销回归通过（target/annotation-pending-cancel-regression.log）。最终门禁 session 73450 仍运行；文字 Release session 1631 仍编译，最早 session 20628 Release 已 exit 0（29m38s）。下一批补序号默认目标/面板退休队列边界，随后恢复刷新率查询和 GPU 热路径测量；不把测试或旧 Release 结果当作最新完整门禁。


最新取消批次 workspace 462/40、Clippy -D warnings 通过；Release session 73450 等待锁，文字批次 Release session 1631 编译中。只读 EnumDisplaySettingsW 查询主显示器 2560×1440、144Hz（target/annotation-display-mode-query.log），接近旧 counter 推算周期但没有绑定性能 fixture output；后续应在平台夹具按拥有窗口查询模式，不能把整数主屏查询当作旧测量的实际刷新率/物理显示证明。


性能夹具已按拥有源/输入窗口查询最近显示模式，记录前后端点并保留独立 OS counter 字段；原生拥有窗口探针显式通过，DISPLAY1 2560×1440/144Hz（target/annotation-owned-monitor-mode-native-final.log）。最终门禁 session 54150 运行中，pending 取消 Release session 73450 编译中；文字 Release session 1631 已 exit 0（14m56s）。构建结束后在安静环境重新测完整高强度模糊夹具，按实际输出模式和 OS cycles 解释 P95 延迟，不以整数配置/小探针代替有效显示或输入到显示验收。UI 序号默认目标与面板退休等待参数回归仍保留。


模式查询第一次完整运行暴露 Backend Drop 后 DPI 上下文变化；已用作用域物理查询与恢复断言修复，原生拥有窗口 cleanup 回归通过。重测 source/input 两端实际 DISPLAY1 2560×1440/144Hz 匹配；60Hz 源下 OS有效更新59.700FPS、GPU P95 2.988ms、dequeue→commit 8.950ms、OS QPC间隔P95仍27.784ms（4周期），20ms/50ms尚未达成。最终门禁 session15751运行；之后继续分解多余周期等待与源发布→候选→系统呈现，UI序号默认目标/面板退休队列和真实App等验收仍未完成。


最终 DPI query 源码 workspace462/41、Clippy -Dwarnings、Windows Release、diff check 均通过；所有当前构建与 native fixture handle 已终止成功。三组安静 Release 基线已更新：OS更新59.700/59.702/58.168FPS，GPU P95 2.988/3.178/4.145ms，QPC间隔P95三组均27.784ms；actual source/input两端 DISPLAY1 2560×1440/144Hz 匹配。高模糊 owned source ordinal3 WGC间隔P95 20.838ms，GPU之外有采集/呈现周期约束；input→visible仍未测。继续冻结版本到呈现的延迟关联与多余周期定位，保留20ms/50ms目标；同时补UI序号默认目标/面板退休队列及实机验收。不再把当前结论描述为 shader性能改善。

冻结源版本到 OS 同步时间已取得有效单 Surface 证据：1,754 样本，dequeue→sync P95 约31.142ms、submit completion→sync 约20.152ms、GPU 2.880ms；更新间隔仍27.784ms，input→visible未测。明确拒绝重复 native count、混合版本和其他 Surface 的来源猜测。workspace465/41、Clippy、Windows Release、diff check均通过，Release session48497已exit0，1m15s。下一步进入序号默认参数目标/面板退休队列回归；性能实验应集中提交到系统呈现的等待，保持现有画质和20ms/50ms目标，不能只优化shader或把统计样本当作真实输入延迟。

序号默认目标回归已发现并修复取消时丢弃合法请求：恢复旧选择后仍按原默认目标执行颜色/编号，不写入旧对象，编号事务与放置撤销分离。完成/取消DPI1/1.25负坐标、退休面板队列清理与旧回调拒绝均通过。当前workspace467/41、Clippy和diff check通过，Release session96974仍活跃（rustc33772编译UI），不可重启或拿旧Release替代。先收尾该门禁，再推进字段级参数更新/协调器边界和提交后等待实验；保留真实App/IME/跨屏、20ms间隔与50ms输入到显示等未验收项。

session96974已exit0，序号目标批次Release2m19s通过。字段合并批次实际回调复现并修复排队线宽覆盖后续菜单颜色的问题；Patch按变化字段合并，回填只更新观察基线。workspace468/41与Clippy通过，Release session26541活跃，rustc13824编译UI。必须继续修复brush_size_scrolled的panel.invoke_values_changed生产模拟调用：Registry借用期间会被新的回填过滤忽略；改走直接参数入口并补回归，然后重新运行最终源码门禁，不拿当前临时绿灯声明完整迁移。保留UI焦点/IME实机、GPU等待与全部性能目标。

滚轮生产调用已改为在设置面板值后获取当前Target并直接apply_geometry_values，不再模拟用户回调；git diff --check通过。该修改发生在session26541的Release编译期间，必须先核对26541终态，再补滚轮回归并重跑最终源码门禁；不能把在途Release当作这条调用方修改的证据。

session26541已exit0，2m18s；滚轮回归已显式通过，覆盖默认/选中笔刷、DPI1/1.25负坐标、直径/效果参数及一次撤销。最终源码workspace469/41、Clippy -Dwarnings通过，Release session54975活跃，rustc3408编译UI。先核对54975终态并补记，然后推进协调器状态边界和GPU提交→系统同步等待；字段级更新的真实IME/DPI/焦点验收与完整20ms/50ms目标仍未完成。不要重启现有构建或引用旧Release替代。

session54975已exit0，最终字段/滚轮批次Windows Release2m18s及diff check通过。已查阅OBS和Win32CaptureSample，并做SyncInterval0/1成对原生实验：0的OS更新间隔P95 27.784ms、dequeue→sync26.007ms；1恶化为34.729ms/38.545ms，未采用。唯一生产改动已恢复0；恢复后Release冻结源像素回归5484已显示通过，须最终核对句柄终态。下一步审计Surface每帧DComp.Commit/SetWindowPos属性操作，以及源发布/系统呈现周期，实验继续一次一个因素且不降低20ms/50ms门槛；结构协调与实机验收仍保留。

5484已exit0。省略每帧DComp.Commit实验完成：更新57.900FPS、GPU3.400ms、更新间隔P95仍27.784ms、dequeue→sync31.302ms，没有明确收益，已恢复Commit。新增ignored自有桌面合成probe（连续4颜色、每帧1点GetPixel精确匹配）在实验和恢复版本均通过；不等于photon/真实App/全帧。当前最终workspace469/42、Clippy通过，Release session2436活跃（平台/App编译），先核对该句柄终态并补记，不能重启。下一步针对SetWindowPos的重复操作成本与源呈现节奏测量，避免直接删除动态层级维护。UI协调/缓存热路径/实机/20ms与50ms目标仍未完成。

2436已exit0，Release1m11s，完整门禁通过。测试诊断已拆分原生提交CPU成本：2,246接受调用样本，Present P95 0.100600ms、DComp.Commit 0.010300ms、SetWindowPos 0.073900ms；同期submit→sync约19.9507ms、OS间隔仍27.784ms。直接CPU阻塞很小，不能凭此认定窗口调用是主要瓶颈或排除系统间接影响。当前workspace469/42、Clippy通过，Release session8634活跃，rustc36648 CPU持续增长，先核对终态再补记。之后推进源发布/刷新周期关联与结构协调，不能把测试诊断当作性能优化或真实input→display验收。

8634已exit0，原生提交分段诊断批次workspace469/42、Clippy、Windows Release、diff check全通过。当前没有待构建句柄；下一批可推进源发布/刷新周期关联或UI协调所有权，保留全部性能和实机验收要求。


参数通知所有权批次最终日志已核对：workspace469/42、最终Clippy和Release成功。参数等待队列与回放规则本批迁入parameters::Pending/ReplayContext，协调器只提供当前上下文并分派应用；最终相关107项回归通过，完整门禁session3649已exit0，workspace469/42、Clippy、Windows Release及diff check成功（target/annotation-pending-replay-*-final.log）。下一批继续参数事务/几何手势交接入口；场景热路径测量、源/系统同步等待、真实App/IME/跨屏和20ms/50ms目标均仍待完成。


事务交接本批已完成：几何开始统一先完成参数事务，手势拥有者统一Core提交/取消，鼠标穿透切换不再重复无条件cancel_drag。新增真实画布按下回归覆盖600ms旧参数闲置任务、DPI1/1.25负坐标、完成/取消及关闭面板；参数与移动独立撤销。完整门禁9615已exit0，workspace470/42、Clippy、Windows Release和diff check通过（target/annotation-gesture-handoff-*-final.log）。后续测量场景签名/长轨迹比较复制，继续收拢协调器行为，保留GPU源/系统呈现等待、20ms/50ms及真实App/IME/跨屏验收。


长轨迹CPU测量与缓存优化本批完成：普通对象缓存命中先判断对象/DPI/参考文字，跳过复制/插值轨迹的边界重算，视口仍为槽键且变更重新生成。显式Release探针100000点cached_scene P95 0.210300→0.058200ms，20000点0.068700→0.011000ms；签名未优化，真实GPU显示未测。完整门禁46720已exit0，workspace470/43、Clippy、Windows Release及diff check通过。下一步继续源帧→系统呈现等待关联，保留20ms间隔未达、50msinput→display未测，以及UI协调、IME/跨屏/真实App等剩余目标。


WGC→OS同步时钟关联验证本批已完成，完整门禁8357 exit0，workspace471/43、Clippy/Release/diff check通过。直接取帧QPC发现当前自有窗口WGC SystemRelativeTime晚于取帧约10–11ms，1785个补充时钟样本全拒绝并输出unavailable；不能以文档单位正确就认定paint起点正确。原有精确源版本dequeue/submit→OS同步样本保留，OS59.502FPS/GPU2.645ms/间隔27.784ms。本批只有诊断，非性能改善。下一步使用可读取自有源版本标记关联真实绘制时钟，禁止最近时间猜测；20ms/50ms、实际App/IME/跨屏、资源和UI剩余重构仍未完成。


像素确认paint→OS同步方法本批通过：BitBlt/GdiFlush区间按唯一出版的四个色带版本匹配冻结源，109拒绝注入后不重复发布。稀疏第一提交原生统计为0/disjoint，改为后续批量精确Count关联，8提交7确认，未确认不猜测。原生64882 exit0，24点误差0、2效果2拒绝；39–66ms是带阻塞读回/拒绝的直接Renderer夹具，不是正式worker/input/吞吐/photon结果。最终门禁75459 exit0，workspace472/43、Clippy/Release/diff check通过。下一步接入持续worker非阻塞版本探针，编码必须不在256帧重复，记录探针成本；20ms/50ms、真实App/IME/跨屏等仍未完成。


持续worker非阻塞paint身份探针已接入cfg(test)：4个复用staging槽、2像素、Map DO_NOT_WAIT、无Flush，24位ID+色带校验避免256帧重复，错误不改变呈现/重建。enabled20236与disabled16783均exit0，1800decode/1794sync samples，paint→sync上界P9540.6412ms、poll CPU0.0013ms，间隔仍27.784ms；关闭对照59.703FPS/GPU2.648ms、启用59.833/2.766，未宣称无开销或input达标。冻结像素最大误差0。完整门禁44804 exit0，workspace473/44、Clippy/Release/diff check通过。下一步对相同paint的真实CPUdequeue→commit→OS同步做分段，源起点以像素身份对应绘制区间，不用WGC未来时间；20ms/50ms及真实App/IME/跨屏等要求仍未完成。


## 当前交接：序号放置所有权已迁移（2026-10-03）

SequencePlacement在现有手势模块内拥有起点、原始对象及插入/更新/完成/取消；路由标记不重复几何状态，序号不再使用通用gesture_object。新按下取消旧未完成放置，关闭释放状态。参数交接、Target/Pending回放及编号/选择恢复语义保留。生产路径回归覆盖DPI1/1.25、负坐标、捕获丢失、工具切换、重入按下和关闭；纯回归覆盖一次撤销及编号耗尽失败释放事务。最终40523 exit0，workspace474通过/44忽略、Clippy -D warnings、Windows Release、diff check通过。未运行真实App/IME/跨屏截图，不将受控Slint回归当作实机证据。

下一批迁移普通对象Move/Resize的起始快照及更新/结束行为，逐步减少Registry通用gesture_object的角色，保留文字角点与编辑草稿协议。窗口焦点、场景热路径和GPU等待继续按计划推进；最新同paint分段paint→dequeue上界P95约15.07ms、dequeue→commit9.58ms、commit→sync上界20.25ms、总上界40.78ms，三缓冲无明确收益且已恢复双缓冲。20ms更新间隔仍失败、50ms真实input→display未测，完整画质/资源/实机要求保持。


## 当前交接：普通对象移动/缩放已迁移（2026-10-03）

GeometryDrag拥有原始对象及移动起点/缩放操作，统一更新、完成、取消；路由标记不再保存目标/起点，Resize仅保留光标控制点。马赛克预览、滤波上下文和未验证对象取消使用同一拥有者，不再拼接通用快照与目标。文字点击进入编辑仍使用真实起点阈值。第二次按下取消旧对象事务，关闭释放拥有者。原gesture_object只剩文字角点，改名text_handle_original并补关闭释放。

标注最终55959 exit0，110通过/1忽略；全量73760 exit0，workspace475通过/44忽略、Clippy -D warnings、Windows Release、diff check通过。回归保留参数/几何交接、撤销与取消、DPI1/1.25、负坐标、铅笔不可编辑、马赛克临时对象、缩放原始快照等。未将自动回归称为真实App/IME/跨屏截图验收。

下一批迁移文字角点的起点、抓取偏移、起始快照及阈值激活状态；必须保留3×DPI阈值、阈值前已接受参数回读、编辑草稿与Core事务分别拥有、旋转锚点和光标焦点协议。随后收拢剩余绘制/折线/箭头手势及统一活动状态；窗口焦点和完整GPU/资源/画质/实机验收仍保持原范围。


## 当前交接：文字角点状态已迁移（2026-10-03）

TextHandle拥有起点、抓取偏移、起始快照、目标、DPI与阈值状态；超过3×DPI前的参数仍可接受，激活时回读已接受对象。编辑草稿不取得Core事务，非编辑文字取得独立事务，取消/完成由真实拥有者执行。移除路由Gesture通用commit/cancel，Registry不再存放文字角点原始对象。已修复未激活取消回退新字号的问题；隔离复现旧规则85366 exit101（33回退22），恢复修复后110标注回归通过/1忽略。最终47375 exit0：workspace475通过/44忽略、Clippy -D warnings、Windows Release2m22s、diff check通过。

下一批收拢绘制、折线和连接箭头的草稿/起始参数/终止行为，并统一活动手势所有权，减少Registry中并列状态与路由标记不一致的可能。保留当前序号、普通移动缩放、文字角点、参数回放、取消和撤销规则。完整窗口焦点、场景热路径、GPU等待、画质/资源及真实App/IME/跨屏截图仍未完成，20ms系统更新间隔和50ms真实输入显示要求保持。


## 当前交接：绘制参数与折线所有权已迁移（2026-10-03）

annotation_drawing的Press拥有起点/按下样式/DPI/箭头移动距离；Draw路由只保留Kind。草稿构建接收借用的参数视图和草稿槽，避免读取Registry或每次移动复制参数。Polyline拥有确认对象、双击时刻、追加/退格/预览和最小点数；ArrowPending拥有两次点击箭头的冻结设置。马赛克上下文使用Press起点，采集失败/工具切换/关闭时释放它。

修复Enter或退格清空折线后迟到松开重新建草稿；生产路径覆盖DPI1/1.25、负坐标、按下期间快捷键、确认节点和一次撤销。最终20170 exit0，workspace476通过/44忽略、Clippy -D warnings、Windows Release2m20s、diff check通过。真实App/IME/跨屏、GPU等待和全部画质/资源验收未完成。

下一批统一活动手势拥有者与路由入口，减少Registry中的sequence_placement/geometry_drag/text_handle/drawing_press和复制路由可产生的不一致。多次点击状态（Polyline/ArrowPending）与显示草稿分别明确生命周期；重点保持对象事务/参数交接、快捷键终止、编辑恢复和马赛克上下文。完成该边界后继续窗口焦点阶段及性能/实机验收，不以拆文件或测试通过代替完整目标。


## 当前交接：活动拥有者与路由已统一（2026-10-03）

Registry只持有一个Option<Active>，互斥拥有序号放置、几何、文字角点、绘制按下或纯动作。四个并列槽及可写复制路由已删除；Gesture从拥有者派生，阈值激活、参数阻塞、光标、编辑转发与马赛克预览即时读取同一状态。类型不匹配的take保持槽，取消只退休实际拥有者并将旧派生路由用于参数回放。新增回归证明错误完成不丢拥有者、纯动作取消不结束/拆分连续参数历史、几何取消恢复文档。

首次20606编译因剩余旧测试字段与借用边界失败，修复后90411标注111通过/1忽略。首轮2619止于测试专用导入的Clippy错误，cfg(test)修复后最终17205 exit0：workspace477通过/44忽略、Clippy -D warnings、Windows Release2m20s、diff check通过。真实App/IME/跨屏及完整性能/画质/资源验收仍未完成。

下一批明确箭头/折线的多次点击等待互斥与显示草稿生命周期，收尾手势边界后进入窗口关闭/返回/焦点模块的行为迁移。保持既有快捷键、参数交接、一次撤销、Core历史与编辑草稿分离，不另建状态框架。CPU热路径、GPU提交等待、20ms间隔/50ms真实输入显示及实机要求继续保留。


## 当前交接：多次点击等待互斥已收尾（2026-10-03）
Registry 的箭头/折线并列槽迁为 Option<drawing::Pending>；typed view 仅提供实际类型的引用。无效箭头终点不消费等待，错误类型完成保持实际拥有者；移出画布时折线显示已确认节点，箭头保持最后预览。切换工具/分组选项及统一取消清理同一槽；删除统一取消之后不可达的两段重复 Escape 分支。光标入口改为派生视图，修复 annotation_cursor.rs:151 E0615。

新增 pending_completion_preserves_other_kinds_and_invalid_endpoints，覆盖 DPI1/1.25、负坐标、无效到有效终点、错误类型完成、预览与已确认节点。保留生产画布快捷键、迟到松开、捕获丢失及一次撤销回归。首轮相关测试47951 exit0；简化取消后最终完整门禁60772 exit0，workspace478通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release和diff check通过。日志：target/annotation-pending-owner-{workspace,clippy,release}-final.log。

本批未修改 GPU 调度，也未执行真实App/IME/双屏截图验收。下一批窗口关闭原因与菜单返回/焦点拥有者迁移；完整CPU/GPU性能、20ms更新间隔/50ms真实输入显示、画质和资源范围继续保留。


## 当前交接：菜单关闭原因已明确（2026-10-03）
annotation_choice::close 必须接收 CloseReason：InteractionFinished、Replaced、ControlPressed、OwnerRetired。同按钮收起、选择/取消完成、显示失败和外部关闭按交互结束归还输入；弹层替换、点击其他控件保留控件输入状态，不调用 release_parameter_input。面板/水印所属窗口销毁、目标/位置变化和语言刷新按退休处理，清理状态但不请求焦点。take 退休菜单的任务后先取消旧恢复请求，只有合法交互完成再请求新的恢复。关闭面板不再依赖“菜单先恢复，面板最后取消”决定行为。

新增实际回调回归 popup_replacement_and_control_press_keep_input_until_interaction_finishes，验证替换不释放输入、工具条按下保持输入、交互完成只归还一次、面板销毁不聚焦退休控件、旧弹层回调无效。相关3698 exit0，114通过/1忽略。完整19561 exit0：workspace479通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m17s、diff check通过。日志 target/annotation-close-reason-{tests,workspace-final,clippy-final,release-final}.log。

这是窗口/焦点阶段的首个行为批次，并非整个阶段完成。下一批审计菜单真实返回所有者：水印返回目前直接 invoke_focus_input，普通文字使用可取消的延迟恢复，应明确各自身份与任务校验；保留预设模板追加、输入法及既有焦点策略。B/C剩余迁移、CPU热路径、GPU等待、20ms间隔/50ms真实输入显示、完整画质/资源及真实App/IME/跨屏截图仍待完成。本批未修改 Slint 界面、Core接口或 GPU 调度，不暂存/提交/推送。


## 当前交接：编辑返回目标与焦点校验已收拢（2026-10-03）
focus::Request 以 Target::Text{editor,panel} 或 Target::Watermark{editor} 绑定真正的输入实例，菜单版本及会话作为共用条件。focus::Context 仅提供必要的活状态摘要；匹配规则移入焦点模块，不读取 Registry。所有恢复均检查新弹层及正在输入/拖动的控件；普通文字额外检查面板身份和水印窗口优先级，水印不依赖工具条实例身份。Coordinator 保存最新请求并支持按目标取消；关闭水印窗口停止其待恢复任务。

菜单关闭按实际所属窗口选择返回目标；水印不释放无关面板输入，finish_return 删除直接 focus_input 的旁路。restore_editor_focus 在操作结束后选择当前水印输入或普通文字实例；水印只调用原有内部 focus_input，不新增原生激活动作，普通文字保留既有激活/光标重置行为。

新增生产回调/受控时钟回归 watermark_menu_returns_to_its_owner_and_retirement_cancels_restoration：预设追加、菜单关闭后的延迟归还、面板更换、控件输入期间不恢复、控件结束返回水印、新弹层/错误会话/错误输入类型拒绝、保留旧组件的关闭重开与任务取消。共用 timer 回归增加跨目标取消不影响另一请求。序号说明回归新增水印打开期间拒绝旧文字请求，关闭后仍在原选区输入。

初步完整85374门禁通过后追加优先级复现；62764 exit101 证明旧匹配会越过水印输入（target/annotation-focus-watermark-priority-negative.log）。首个替换脚本因格式未匹配断言失败，未应用修复；随后补齐实际条件。最终66245相关115通过/1忽略；完整27468 exit0：workspace480通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m20s、diff check通过。最终日志 target/annotation-focus-target-{tests,workspace,clippy,release}-accepted.log，以 accepted 批次为当前源码证据。

下一批审计外部/前台变化关闭时的激活策略：当前普通文字仍沿用激活请求，原生 DismissRequested 无关闭原因，需区分局部输入恢复与明确用户激活，防止跨应用抢焦点；不以受控 Slint 测试宣称真实 App 该项已验收。窗口/菜单剩余迁移、CPU热路径、GPU等待、20ms更新间隔/50ms真实输入显示、完整画质/资源和真实App/IME/跨屏截图仍保持原范围。本批不修改Core接口、配置、依赖或GPU调度，不暂存/提交/推送。


## 当前交接：原生关闭原因与局部恢复已接线（2026-10-03）
现有 PopupDismissMonitor 保留原来的鼠标/前台监控机制，私有窗口消息携带 PointerOutside 或 ForegroundChanged，桥接和 App 映射保留原因到 UI。没有新增 Core 公开接口、crate、配置或依赖。翻译语言菜单、托盘菜单、翻译弹窗继续处理两种关闭事件。标注三级菜单和工具分组菜单仅对 PointerOutside 使用按钮/菜单位置防误关判断；前台变化关闭不再因光标停在弹层上而被忽略。二级参数面板继续不注册自动关闭，收到任意原因均不退休。

focus::Request 增加 Activation::Local/UserRequested。外部关闭与菜单准备失败恢复内部输入，不调用原生激活；明确交互结束保持现有普通文字激活。水印仍只内部 focus_input。菜单关闭原因中补充 ExternalDismissed/PresentationFailed，替换、控件按下和所属窗口退休规则不变。

新增实际 Slint 回调/受控时钟回归 foreground_dismissal_closes_the_menu_without_activating_the_text_window：鼠标停在三级菜单上仍关闭，二级保留、编辑身份和选区不变，随后输入替换原选区，原生激活计数保持0；明确菜单选择后激活一次。扩展二级/分组菜单回归覆盖两种关闭原因，保留DPI1/1.25及旧窗口事件拒绝。Platform 回归直接执行私有消息的真实 subclass 分支，检查两种原因及未知编码拒绝；它不等于实际 HWND 消息/全局钩子/Alt+Tab实机验收。App回归验证原因映射。

相关2466 exit0：UI116通过/1忽略、Platform18通过/1忽略、App映射1通过。最终57978 exit0：workspace483通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m15s、diff check通过。日志 target/annotation-dismiss-cause-{ui-tests-final,platform-tests-final,app-tests-final,workspace-final,clippy-final,release-final}.log。

本批关闭了外部 dismissal 路径上的原生激活，不宣称跨应用焦点全部验收：明确操作结束请求排队后发生前台切换的竞争仍需审计来源和执行时授权，不能只用当前编辑身份替代。真实App的Alt+Tab、IME、双屏、负坐标和截图仍待执行。下一批继续该请求竞争/窗口任务来源边界，随后推进场景CPU热路径与GPU等待；20ms更新间隔/50ms真实输入显示、完整画质/资源与全范围实机验收保持。本批未修改GPU调度，不暂存/提交/推送。


## 当前交接：参数确认与失焦已分离（2026-10-04）
AnnotationPanel.parameter-input-finished 携带 confirmed 标记；所有数字控件通过共用 confirm-parameter-input/finish-parameter-input 转发。Enter和增减确认维持UserRequested，普通失焦使用Local恢复，不调用原生激活。面板 confirmed-input-exit 将确认后释放控件产生的延迟blur视为同一次操作；再次进入输入控件时重置，普通后续失焦正常提交。未改变布局、参数规则、Core接口、配置或依赖。

扩展真实 Slint 数字输入回归：鼠标进入字号输入区、实际Return按键、自动失焦、再次进入后显式释放输入区。首次78131 exit101：确认被自动blur覆盖，原生激活计数1而预期2（target/annotation-input-completion-tests.log）。修复确认退出去重后88433通过；受控渲染首轮因Rgb8Pixel导出路径编译失败，改用slint::Rgb8Pixel，18973通过。按需渲染仅由LEXIFT_ANNOTATION_RENDER_DIR测试环境变量启用，使用MinimalSoftwareWindow真实软件renderer输出PPM并转换PNG，无新增依赖；这是受控渲染能力，不是实际App截图或生产配置。

已查看 target/annotation-input-completion-render/numeric-input-blur.png，900×58，单行按钮、字体/字号、九色板完整显示。它只证明当前受控布局，没有证明系统焦点或实际IME/双屏。最终11611 exit0：workspace483通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m16s、diff check通过（target/annotation-input-completion-{workspace,clippy,release}-final.log）。Slint由UI build.rs全量编译。

明确UserRequested请求在排队后发生前台变化的竞争仍待处理：下一批应保存请求发出时的授权/前台观察，并在执行时重新核对，不能执行时才替旧事件赋予新来源。优先复用注入能力和现有窗口token，避免新全局hook或Core公开接口。仍保留窗口阶段其余验收、真实App/Alt+Tab/IME/跨屏截图、CPU场景热路径、GPU等待、20ms更新间隔/50ms真实输入显示以及完整画质/资源目标。本批不暂存/提交/推送。


## 延迟焦点恢复来源校验（2026-10-04）

新增UI内部focus::Origin，区分未注入观察与事件发生时的前台窗口。App注入Platform只读foreground token查询，Windows复用现有GetForegroundWindow能力，不新增全局hook、Core接口、配置或依赖。数字确认、滑杆结束、原生工具条鼠标松开和菜单选择在原始回调中保存观察，再进入InputWork；菜单延迟返回的ReturnToken保留该来源。焦点计时器执行时，前台必须仍为原观察窗口或此次恢复安排时的明确标注窗口token，才保留原生激活；否则降为Local，仅恢复既有Slint输入，不激活窗口。既有会话/编辑/工具条/弹层身份校验继续执行。

扩展生产Slint回调回归：确认进入队列前后切换前台、计时器前切换、未变化的明确操作、菜单选择排队后切换、弹层退休后返回准确标注所属窗口；纯函数覆盖无前台及无能力观察。首次脚本UTF8读取失败，没有应用测试；target/annotation-focus-origin-negative.log仅为旧测试通过，不能用作负向证据。修正编码后73201 exit101，negative-applied.log证明确认排队后仍激活（计数3，预期2）。实现后定向4255 exit0；菜单扩展后2785 exit0，标注117通过/1忽略。首轮完整58145 exit101：foreground_token的pub(super)无法供crate根调用；改为pub(crate)。最终30347 exit0，workspace484通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check通过；UI build.rs编译Slint。最终日志target/annotation-focus-origin-{workspace,clippy,release}-accepted.log。

本批没有改动Slint视觉布局，没有新增截图，受控前台观察和原生激活计数不等于真实Alt+Tab/IME/双屏验收。当前token观察只证明执行时窗口身份，并非完整前台变迁历史；切走又回到同一窗口的情况尚未验证。参数菜单再次点击收起的on_choice_requested仍先排队再open，需继续把原始来源传入open/close；Escape及画布输入排队入口也需核对。原生panel当前在所有转发事件查询来源，后续收敛到实际需要恢复的松开事件，避免无关移动上的查询。

下一批完成上述入口和来源边界回归，再推进场景CPU热路径、GPU等待、完整画质和资源。20ms更新间隔/50ms真实输入显示目标及真实App/跨屏范围保持，不宣称整个窗口阶段或重构完成。不暂存、提交或推送。


## 菜单收起、Escape与输入窗口准备来源（2026-10-04）

on_choice_requested在原始Slint回调捕获Origin，传给choice::open_from/open_menu；同按钮收起使用close_from，父面板返回继续携带ReturnToken来源。Escape经escape_from/dismiss_from处理，主工具条、参数面板、画布的延迟入口均在排队前保存来源；参数窗口关闭请求使用相同规则。Escape关闭二级后激活主窗口之前也检查来源。工具条拖动在排队前捕获来源，关闭菜单后只在授权仍有效时开始原生拖动，避免旧拖动请求抢占前台。

文字初次编辑同步取得来源，并贯穿show_text_editor的全部准备重试；水印编辑按钮在InputWork之前取得来源，open_watermark_input_from及全部准备重试保留它。原生准备成功后，明确激活调用受来源/当前标注窗口身份校验。画布指针回调与编辑指针回调同步处理，无额外前置InputWork；其焦点计时器仍使用前批来源校验。panel原生鼠标转发只在LeftReleased查询前台，移除所有悬停移动上的查询。已迁移的无来源open/dismiss/escape包装仅作为cfg(test)辅助，生产调用全部使用带来源入口，不用allow(dead_code)掩盖旧路径。

扩展真实Slint回调/受控时钟回归：同按钮收起排队后切换前台、Escape仅关闭一级菜单且保留编辑实例、文字准备Pending到Ready时切换前台、水印初始请求排队及准备重试后切换、合法新请求仍激活；100次原生panel Moved事件查询计数为0。15058 exit1负向证明确认后的菜单收起仍激活（计数5，预期4，target/annotation-focus-entry-negative.log）。首次接线127e96编译失败：水印已有PhysicalPosition origin局部名遮蔽了请求来源；改为request_origin。25003定向通过；补充准备/悬停回归后98276通过。第一轮完整71910 workspace通过、Clippy exit101发现旧escape无生产调用；将其收为测试辅助。最终21678 exit0：workspace484通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release及diff check通过。Slint由build.rs编译，未改视觉布局，没有新截图。日志target/annotation-focus-entry-{workspace,clippy,release}-accepted.log。

本批不证明真实Alt+Tab、IME、DPI、跨屏或完整前台历史。切走又回到相同token的历史问题仍未验证；原生show是否始终维持被动展示也需实机确认，不能只凭激活回调计数宣称整个窗口阶段完成。CPU场景、GPU调度、20ms更新间隔/50ms真实输入显示、完整画质与资源验收继续保持范围。不修改Core接口、配置或依赖，不暂存/提交/推送。

下一批进入阶段3的明确测量：沿用Release隔离的long_trajectory_signature_and_cache_costs，扩展到多视口/长笔刷、文档快照和光标仅覆盖层变化的真实准备路径；先记录指纹、完整比较、复制、场景编译和分配成本，再选择重复工作最多的环节。避免直接给部分Core写入增加版本，避免把视口指纹当全局文档身份。与GPU调度使用独立基线，后续仍回到窗口实机和资源验收。当前无运行中的构建，窗口来源接线批次已完整门禁。


## 马赛克遮罩几何与跨视口共享（2026-10-04）

阶段3先运行当前单轨迹Release隔离基线：13665 exit0，100k点单视口指纹P95 0.3952ms、缓存场景0.0550ms（target/annotation-cpu-baseline-current.log）。源码审计发现SceneCompiler::layers在每帧、每视口先clone整条马赛克轨迹并重算work_regions，之后才查询遮罩栅格缓存；各遮罩分块还分别拥有轨迹副本。新增long_mosaic_native_preparation_costs，直接调用生产SceneCompiler::compile，两相邻物理视口含负坐标、scale1/1.25，已完成笔刷取消选中，隔离测量场景准备、四次文档/展示指纹和不可变快照复用。240样本、1k/20k/100k点，测量不包含采集/GPU/原生提交/系统同步或真实App延迟。缓存诊断仅统计不同points缓冲区的capacity字节，不是PrivateBytes、RSS、纹理或完整分配量。

| 点数 | 场景准备P95前→后(ms) | 四次指纹P95前→后(ms) | 缓存轨迹缓冲区前→后 | 点缓冲区容量字节前→后 |
|---|---|---|---|---|
| 1k | 0.0419→0.0093 | 0.0196→0.0398 | 9→2 | 72000→16000 |
| 20k | 0.7446→0.0254 | 0.3450→0.3168 | 9→2 | 1440000→320000 |
| 100k | 4.0476→0.1214 | 1.7464→1.7298 | 9→2 | 7200000→1600000 |

前后均同夹具240样本；小样本指纹波动和快照差异不作为优化收益，指纹与快照实现未改。基线26039 exit0（annotation-mosaic-native-cpu-baseline.log），优化后22148中首个命令exit0（annotation-mosaic-native-cpu-after.log）。100k场景准备约33倍改善，缓存点容量减少约78%；这不等于屏幕跟随性能达标。

实现：新增UI内部annotation_scene_mosaic::Geometry，拥有一份规范化Arc<Object>及按物理视口缓存的work_regions输出。按对象身份、草稿索引区分；每批编译用完整借用字段比较识别几何变化，所有写入/撤销恢复自然经过同一比较，不新增部分版本机制。Object、Style、MosaicAnnotation使用穷尽字段模式，新增字段会暴露编译检查；规范化字段明确排除颜色、线宽、效果和强度，其他几何/样式保守参与。白色遮罩各分块共享同一Arc，不再各自clone轨迹；采样元数据仍保留独立真实Object，效果/强度修改照常回填GPU场景。仅减少重复几何工作，绘制和分区算法保持现有实现。

EffectRaster与普通ObjectRaster分开：前者持有共享Arc，ptr_eq快速复用同一规范化形状，后者保留原有普通标注缓存。视口移除清理对应分区和栅格，无剩余视口时释放几何；关闭效果、对象不可见/删除、会话clear同样清理，不保留旧会话。没有Core公开接口、依赖或配置变化。

79347初轮场景9通过；扩展34329场景10通过。新回归mosaic_geometry_shares_trajectories_and_invalidates_complete_masks用完整像素比较验证跨视口、负坐标、1/1.25尺度、参数仅更新、轨迹/直径/creation_scale/rounding/外形变化及先前对象快照恢复；断言所有分块共享轨迹、缓存仅两份points、移除视口的Weak释放、强度0/删除/clear清理。已有层序、普通文字、旋转、编辑覆盖层和分区画质回归保留。22148最终完整门禁exit0：workspace485通过/45忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release及diff check通过；Slint由build.rs编译。日志target/annotation-mosaic-geometry-{workspace,clippy,release}-final.log。没有运行中构建。

本批无视觉UI改动/实机截图，完整像素证据只对应测试遮罩，不是全App输出/实时屏幕画质。尚未覆盖生产App的IME/Alt+Tab/双屏、完整资源压力或真实输入显示延迟。GPU调度未修改，20ms系统更新间隔未达标、50ms真实输入显示未测继续保留。下一批根据1.73ms四次指纹成本，审计同帧文档/展示/效果签名的重复遍历，优先帧内借用复用，不引入跨帧漏失效的对象版本；快照保持完整文档身份。CPU与GPU调度分别测量，随后推进GPU等待及完整实机/资源验收。不暂存、提交或推送。


## 帧内指纹复用与借用手势上下文（2026-10-04）

新增UI内部annotation_signature::Frame/Content/Replacement，将旧signature_for_objects具体行为移出会话协调器。Frame借用固定Document及草稿，按文档索引缓存内容指纹；Vec<Option<u64>>仅存在于本轮渲染，未存入Registry或后台任务。所有内容写入、撤销和下一轮编辑都自然获得新Frame，不新增跨帧对象版本或Core接口。可见性检查保留旧边界/旋转文字/放大镜输出规则；指纹组合显式加入物理视口边界，视口变化不再仅依赖可见对象集合。内部64位组合布局改变，不是持久化格式。

软件路径的马赛克任务签名、各画布展示签名及效果反馈raster_signature_with共享同一Frame。对象内容（含轨迹）最多计算一次，水印/聚光背景公共内容也只计算一次；临时编辑替换与草稿以固定索引区分。后台Snapshot仍比较完整文档、显示元数据及水印，不使用帧内指纹冒充完整身份。原生已活动路径仍在该软件签名批次之前返回，本次不宣称GPU等待或显示延迟改善。

移动/缩放上下文用Replacement借用起始bounds/points并预计算一次替换指纹，保留当前参数及必要的起始直径；绘制起始上下文用栈上单点数组，不再clone增长中的轨迹再丢弃。原帧对象memo不被替换污染，其他对象继续复用本轮键。效果类型/强度、字号/字形等完整内容仍经穷尽字段指纹，Snapshot/工作线程协议保持原有完整比较。仍有预览状态等其他轨迹复制，不能宣称所有热路径复制已消除。

58769标注117通过/2忽略。新增3项Frame回归首轮编译失败：Session没有Clone；使用显式测试文档投影并保留借用对象，移除旧协调器Hash导入。25212中3项通过；其worker筛选annotation_mosaic_worker实际选中0项，不能作为worker证据。修正mosaic_worker筛选后27234中15项通过，覆盖层序、采样前缀、移动/缩放反馈、跨屏透镜、分区边缘等；最终workspace还执行全部相关测试。新增3项覆盖可见对象只计算一次、不同DPI/视口、参数及快照恢复、借用几何替换不污染memo、编辑对象及草稿的不同索引和内容一致性。

沿用同双视口Release夹具、240样本，四次逻辑签名改为生产的共享批次，计入Frame构造；每次断言只有一个对象内容被计算。前批基线target/annotation-mosaic-native-cpu-after.log；本批target/annotation-frame-signature-cpu-after.log：

| 点数 | 四次签名P95前→后(ms) | 本批原生场景准备P95(ms) | 快照复用P95(ms) |
|---|---|---|---|
| 1k | 0.0398→0.0050 | 0.0039 | 0.0006 |
| 20k | 0.3168→0.0994 | 0.0247 | 0.0106 |
| 100k | 1.7298→0.7020 | 0.1148 | 0.0565 |

原生场景准备和快照实现未改，本批不把其波动作为收益。缓存轨迹仍2份，100k点容量仍1600000字节；帧内键表是按对象数分配的短期Vec，并非轨迹副本。仍有一次线性内容指纹和完整文档比较，未验证全App/分配压力或实际输入显示。

最终27234 exit0：workspace488通过/45忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m14s及diff check通过；Slint由build.rs编译，无视觉Slint布局变更和新截图。日志target/annotation-frame-signature-{workspace,clippy,release}-final.log。无运行中构建，不暂存、提交或推送；未改Core接口、配置、依赖或GPU调度。

下一批进入独立GPU调度诊断：检查现有FrameArrived/arrival通知到渲染worker等待的接线、来源版本及交换链permit是否造成额外整周期等待；优先复用平台内部信号和会话所有权，不新增Core公开接口。保持既定同源像素/画质验证和单因素对照，无收益实验撤回，遇到真正难点再查官方文档和适用优秀开源实现。CPU剩余预览复制、完整资源/画质、真实App/IME/Alt+Tab/DPI/跨屏和20ms间隔/50ms真实输入显示目标继续保留，未将阶段3或整体目标标记完成。


## 新场景唤醒与 GPU 调度拥有者（2026-10-04）

审计确认 FrameArrived 已经通过 Notice/Signal 唤醒 worker；不需要重复接线。原 worker 只在当前采集 backend 的 arrival 时提前 next_attempt，新场景提交虽然唤醒事件，但随后仍等待原 60Hz 截止时间。新增 Platform 内部 scheduler::Cadence，拥有常规截止时间、下一次尝试及最后尝试的 RenderStamp；新场景仅提前一次，同一未完成场景仍节流。保持常规定时相位、丢弃错过周期、500us Source 重试、最新场景合并、当前 backend 通知及交换链许可。会话代际清理重置调度；有限设备重建和前帧交接保持原协议。没有新增 Core 公开接口、配置或依赖。

新增三项确定性回归覆盖新版本提前一次/最新版本合并/相位保持、来源到帧与重试节流、错过周期/代际重置/新会话。定向 annotation-gpu-cadence-tests.log 为3通过/1忽略。Attempt 的 stamp 仅用于测试诊断，生产不增加测量记录。

新增 Release 自有静止 WGC 源夹具 gpu_static_source_scene_request_to_attempt：初帧就绪后不再绘制来源，只提交200个新场景版本，以1–15ms间隔覆盖不同定时相位；按完整 stamp 匹配首次 worker 尝试。baseline 临时恢复旧的“仅 arrival 提前”策略，仅运行该夹具，随后恢复最终策略再运行。两次进程均 exit0：

| 指标 | 旧策略 | 新策略 |
|---|---|---|
| 样本 | 200 | 200 |
| 请求到首次尝试 P50(ms) | 7.1393 | 0.0214 |
| P95(ms) | 14.4016 | 0.0485 |
| 最大值(ms) | 16.4579 | 0.9682 |

日志 target/annotation-gpu-scene-wake-{baseline,after}.log。该夹具使用同内容的新场景 stamp，证明 worker 调度响应；没有真实输入，也未测移动后提交/系统同步/物理显示，不据此宣称整条显示延迟达标。最终源码已恢复新策略，完整 workspace 执行上述确定性回归。

实时35秒大区域模糊回归通过：源60.002FPS，提交59.969FPS，GPU P95 2.857ms，匹配来源的系统显示57.635FPS，系统 QPC 更新间隔 P95 27.783ms（target/annotation-gpu-cadence-throughput.log）。本批没有改变固定场景的来源更新调度，间隔目标仍未达到；不把该单次吞吐波动归因于优化。原生像素版本夹具通过4个版本/2种效果/2次候选拒绝，每帧24个抽样点最大通道误差0（annotation-gpu-cadence-pixels.log），不等于完整 App 画质或吞吐延迟测量。

第一轮33689 workspace通过，Clippy exit101发现新增测试的常量 assert 写法；改为既有 if cfg + panic 模式，没有抑制 lint。最终87596 exit0：workspace491通过/46忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check通过，Slint由UI build.rs检查。日志 target/annotation-gpu-cadence-{workspace,clippy,release}-accepted.log。没有运行中构建；无视觉布局改动/新实机截图，不暂存、提交、推送。

下一批继续阶段4采集新鲜度、交换链许可及系统同步的独立诊断，保留同来源版本关联与单因素对照。更新间隔20ms未达标、真实输入显示50ms未测、完整App/IME/焦点历史/DPI/双屏/负坐标/画质/资源，以及剩余协调器草稿交接和CPU复制范围仍保留，整体重构未完成。


## 输出确认与来源准备分阶段（2026-10-04）

根据87596后的现场夹具诊断，Presentation等待9次，Source等待9152次，约399次尝试/秒；采集窗口枚举在输出候选确认重试时仍重复执行。没有绕过交换链许可。Renderer::render现在先poll已冻结的输出候选，Pending/Equal/Changed直接走resolve_output；仅Compose进入来源采集、效果准备与合成。候选只会在后面的stage创建，删除原先准备完成后的第二次poll和恒成立Compose分支。没有新增平行状态/通用事件总线/Port/配置/依赖。

旧候选拥有后缓冲，不能被新输出覆盖；确认/提交结束后明确返回Source等待，使用已有500us重试及时处理积累的最新来源。新来源在确认期间暂留采集队列，不冒用旧像素确认新来源；原有版本校验、等像素不提交、提交拒绝保留候选、有限设备恢复均保留。output.rs最终没有新增has_candidate接口，实验阶段的冗余访问器已移除。

新增测试诊断：Stages.output_retry及每秒累计worker/capture工作耗时。它们仅在cfg(test)中使用；工作耗时是attempt观察到的elapsed总和，包含该夹具的诊断及调度影响，不是进程CPU利用率、线程CPU计数、真实输入显示或GPU着色器时间。

### 实验与对照

初试annotation-output-retry-throughput.log：GPU P95 1.893ms，但显示53.236FPS、间隔34.729ms；因此未凭GPU时间更短直接接受。补充同源60Hz、同设备、35秒夹具的受控前后运行：

| 指标 | 旧顺序 / controlled-baseline | 优先确认 / controlled-after | 整理后实现 / output-phase-throughput |
|---|---|---|---|
| worker累计工作ms/秒 | 237.675 | 213.708 | 199.424 |
| capture累计工作ms/秒 | 208.913 | 175.361 | 160.044 |
| 仅输出确认重试次数 | 0 | 7343 | 7196 |
| GPU P95(ms) | 2.731 | 2.950 | 2.589 |
| 来源取帧到提交P95(ms) | 9.222 | 8.935 | 8.859 |
| 匹配来源系统显示FPS | 57.099 | 59.136 | 59.502 |
| 系统QPC间隔P95(ms) | 34.728 | 27.783 | 27.784 |

同设备NVIDIA RTX4060 Laptop、来源约60FPS。工作量下降在两次运行中出现；运行波动及首轮53FPS说明不能据此宣称所有运行环境都更快，也不把不同运行中的间隔差异当确定因果。保留该阶段改动的主要依据是实际重复枚举减少与工作耗时对照，配合版本/画质回归。系统间隔20ms仍未达到。

四层重叠output-phase-overlaps：worker200.917ms/秒，capture154.375ms/秒，GPU P95 3.839ms，提交59.934FPS、匹配系统显示59.801FPS、间隔27.784ms。本次没有同批四层旧顺序对照，不声称该场景的改善倍数。基线/实验日志 target/annotation-output-retry-controlled-{baseline,after}.log；最终大区域与重叠日志 target/annotation-output-phase-{throughput,overlaps}.log。

### 回归与门禁

扩展已有原生D3D11回归gpu_single_source_change_survives_unready_surface_and_rejected_commit：未就绪/拒绝提交保留旧候选；确认重试capture耗时为0；新来源保持可取，确认后请求Source快速跟进；下一轮显示新来源且清偿其债务；新场景在比较期间替换旧版本，最终只提交一次并完整读回4×4覆盖层像素。初轮14086因旧测试假设“确认时已准备新来源”失败，改为验证新阶段的队列保留及即时跟进，未删除新来源必须显示的检查。首轮2380的新增像素断言因RGBA输入/BGRA读回混用失败，修正测试预期，未改变生产颜色规则。

最终原生回归通过；自有WGC版本像素夹具4版本/2效果/2次候选拒绝、每帧24抽样点最大通道误差0。后者不是完整App画质或吞吐测量。最终20236 exit0：workspace491通过/46忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check通过，Slint由UI build.rs检查。日志target/annotation-output-phase-{regression,pixels,workspace,clippy,release}-accepted.log。没有运行中构建、视觉UI改动或新增实机截图，不暂存/提交/推送。

### 外部参考与剩余范围

核对了[DXGI等待对象官方说明](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_3/nf-dxgi1_3-idxgiswapchain2-getframelatencywaitableobject)，继续保留绘制前许可。查阅[Win32CaptureSample的SimpleCapture](https://github.com/robmikh/Win32CaptureSample/blob/main/Win32CaptureSample/SimpleCapture.cpp)：CreateFreeThreaded到帧回调取帧后复制并Present1，缺少本项目的效果/输出相等比较阶段，不能直接复制其时序以假定性能相同。OBS文件页面没有提供本轮所需的明确对应实现，没有据此导入方案。

下一批继续来源到帧新鲜度、许可和系统同步的独立时序诊断。保留20ms系统间隔未达标、50ms真实输入显示未测，以及真实App/IME/焦点历史/DPI/双屏/负坐标/完整画质/资源释放、协调器草稿交接和剩余CPU复制范围。整体目标未完成；测试门禁不替代实机证据。


## 刷新周期参考与同像素版本时序复测（2026-10-04）

本批不修改生产调度或效果算法，补齐下一项性能决策的证据。display_timing新增测试内部ideal_periodic_intervals：4096个周期来源版本映射至首个可用刷新，超过刷新率的来源合并为显示刷新，禁止非正/非有限周期。参考模拟忽略采集/处理、来源抖动、VRR及丢帧，不作为硬件下界或验收门槛。确定性回归验证60Hz来源/144Hz显示的P95为20.833ms，约40%间隔超过20ms；120Hz显示为16.667ms；240Hz来源/60Hz显示不会生成零间隔。保留既定20ms目标，不静默放宽或用参考模型假定不可优化。

生产worker夹具报告按实际来源平均周期和从OS QPC/刷新计数推得的显示周期输出参考P95、参考及实测超过20ms的比例、实测超出参考P95+0.1ms的比例。原来的实际间隔、完整像素来源关联和门槛不变。首次接入脚本因存在三个eprintln报告点而拒绝插入，只有display_timing文件写入；其确定性测试通过，第一次延迟运行没有参考报告。之后明确锚定refresh_cycle_histogram后的报告位置，重新测量并完成门禁，没有把首次日志当最终诊断。

两次35秒同像素ID自有源复测：

| 指标 | output-phase-paint-latency（首次） | refresh-reference-paint-latency（最终） |
|---|---|---|
| 完整匹配样本 | 1764 | 1789 |
| 绘制到取帧上界P95(ms) | 15.0279 | 16.2531 |
| 取帧到提交P95(ms) | 11.0588 | 8.3973 |
| 提交到OS同步上界P95(ms) | 20.1432 | 20.1910 |
| 整段上界P95(ms) | 40.7779 | 40.9065 |
| GPU P95(ms) | 3.251 | 2.592 |
| 匹配系统显示FPS | 58.835 | 59.668 |
| 系统QPC间隔P95(ms) | 27.784 | 27.784 |

每个延迟样本通过实际像素ID绑定源BitBlt/Flush、取帧、提交及系统同步；各段P95不相加，OS同步不是物理显示/真实输入。不同运行存在波动，没有以第二次某段下降宣称生产优化。最终刷新周期6.946ms，参考P95 20.837ms；参考超过20ms比例0.399658，实测0.395752，实测超过参考P95+0.1ms比例0.230296。实际直方图{1:428,2:653,3:296,4:373,5:37,6:2}说明更多短周期补偿与较长间隔并存，不能将39.6%与40%接近解读为已达最优，不能简单把P95相减当额外延迟。4+周期长尾仍存在。

最终81096 exit0：workspace492通过/46忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release和diff check通过；UI build.rs覆盖Slint。日志target/annotation-refresh-reference-{tests,paint-latency,workspace,clippy,release}.log。未新增Core公开接口/配置/依赖，不暂存/提交/推送，没有运行中构建。仅测试诊断代码变化，无新实际App截图。

下一项优先核验同像素版本在输出确认与源到帧之间的相位/新鲜度，而不是无依据增大队列或绕过许可。可继续针对采集比较重试的重复窗口枚举做保持窗口集合变化及时可见的独立方案；不能因省CPU而失去新窗口/遮挡/尺寸变化。CPU余下轨迹复制、草稿交接、窗口实机、完整画质/资源和全部原阶段仍保留。20ms间隔未达标、50ms真实输入显示未测，完整目标未完成。


## 采集裁剪计划复用与像素夹具释放顺序（2026-10-04）

不缓存桌面窗口集合：sample/sample_gpu仍逐次EnumWindows、检查可见/最小化/cloaked/原生标记/排除名单、窗口边界、透明度及Z顺序。避免为了减少枚举而延迟新窗口或遮挡变化。本批将已有disjoint_crops纯算法移至screen_capture/crops.rs，新增Backend拥有的Plan：按完整request.regions更新公共来源，每个token存最后bounds及裁剪矩形Vec；请求区域变化清空全部计划，边界变化替换单窗口计划，当前枚举不可见的token立即移除。CPU/GPU两条采集路径共用，不重复创建裁剪Vec；paused/excluded变化仍经实时窗口枚举和后续既有逻辑处理。stop_capture清空，Backend销毁随所属会话释放。裁剪/并集算法本批没有改变，没有新增Core公开接口/依赖/配置。

新增纯回归验证重复请求100次保留同一Vec地址、负坐标交叠区域的精确原算法输出、窗口移动/尺寸、隐藏移除、请求变化、旧区域恢复及clear。screen_capture相关12通过/4忽略（annotation-crop-plan-tests.log）；旧透明窗口并集回归保留。

隔离Release benchmark使用2/16/64个斜向交叠区域，240个样本，以旧纯函数和Plan.retain/get直接对照，black_box避免消除计算，计入缓存查找/保留而不计GDI/WGC/GPU/显示。初轮11445在64区域的240×100重复计算长时间运行；检查具体自有测试进程20688命令后主动停止，11445终止exit-1，不是通过。64区域调整为每样本1次（2/16仍100次平均），保持240样本与算法/区域不变。67857中的成本步骤通过，annotation-crop-plan-cost-accepted.log：

| 区域数 | 每次原计算P95(ms) | 复用P95(ms) |
|---|---|---|
| 2 | 0.000200 | 0.000018 |
| 16 | 0.219255 | 0.000023 |
| 64 | 15.716300 | 0.000400 |

这些仅证明重复几何成本，不能换算整屏跟随延迟或进程CPU。64区域首次/变化后的冷计算仍约15.7ms，已发现原逐块减法碎片增长瓶颈；后续单独优化精确矩形并集。Plan缓存仍保存原算法矩形结果，不宣称所有分配/内存问题解决。

67857原生像素夹具完成全部像素断言后停在退出清理，日志没有test result，不算通过。确认进程32332仍live，CPU约0.52秒、111等待线程，原夹具Renderer先创建而owned source/input后创建，反向局部析构会先销毁捕获中的HWND。没有原生stack证据，不能把这个顺序推定为停滞唯一根因。主动终止该具体自有进程（67857终止exit-1）；在夹具末尾明确stop_capture、drop验证资源和Renderer，使生产者/呈现先于source/input HWND退役。它是测试夹具清理顺序修正，未扩大生产窗口策略。

修正后32899的像素夹具正常退出：4来源版本/2效果/2次候选拒绝、每帧24点最大通道误差0，0.92秒；不证明所有资源释放或完整App画质。最终32899 exit0：workspace493通过/47忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check通过；Slint由UI build.rs检查。日志target/annotation-crop-plan-{pixels-accepted,workspace,clippy,release}.log。无运行中构建，无新实际App截图，不暂存/提交/推送。

下一批优先处理已测明的冷裁剪算法增长，要求精确覆盖、不重叠、不把间隙采入、不双重合成透明窗口，以及负坐标/极端边界/区域排列与恢复一致；采用独立性能基线，不借此放宽实时效果目标。GPU来源新鲜度/系统同步、20ms间隔、50ms真实输入显示、剩余协调器/草稿/CPU副本、实机IME/DPI/双屏/负坐标/焦点历史、完整画质与资源范围继续保留。


## 冷裁剪精确并集算法（2026-10-04）

针对上一批64交叠区域首次裁剪约15.7ms，生产disjoint_crops改为横向条带：先与窗口bounds求交，排序去重所有Y边界；每条带排序合并X区间，前一条带中完全相同的X区间向下延伸。使用整数比较，不逐像素扫描、不填间隙、不重复合成透明窗口；触边区间可以合并。区域排列与重复不改变输出顺序/覆盖，避免缓存因输入顺序产生碎片变化。原逐块减法算法只保留为cfg(test) reference_crops，用于独立旧实现对照，生产没有平行路径。Plan生命周期及实时窗口枚举保持上一批规则，不新增Core公开接口、依赖或配置。

新增回归：40组确定种子、0–39矩形的负坐标/裁剪/交叠/间隙样本，在24×24完整网格上对照原始区域集合、旧算法和新算法，逐像素必须覆盖恰好一次；反转输入和追加重复矩形得到相同新输出。极端i32::MIN/MAX边界、触边水平/垂直合并、空集合、零宽窗口，无坐标加减溢出。旧separate_crops透明合成/空隙回归继续通过。相关screen_capture14通过/5忽略（annotation-slab-union-tests.log）。

同夹具240样本，2/16区域每样本100次，64区域每样本1次，旧/新纯函数及Plan命中同进程交错测量；范围是隔离ROI几何，不含GDI/WGC/GPU/显示。annotation-slab-union-cost.log：

| 区域数 | 旧算法冷P95(ms) | 新算法冷P95(ms) | Plan命中P95(ms) |
|---|---|---|---|
| 2 | 0.000321 | 0.000374 | 0.000019 |
| 16 | 0.259763 | 0.003722 | 0.000025 |
| 64 | 17.836200 | 0.026600 | 0.000300 |

16/64区域冷计算显著下降；2区域没有改善，差异约0.053us，不作为收益。没有给小数量另设算法旁路，也没有把局部计算收益换算为整屏跟随或进程CPU利用率。算法仍有按Y条带遍历全部区域/排序/分配的成本，未宣称任意数量都无成本。

原生像素版本夹具正常退出：4来源版本/2效果/2候选拒绝，每帧24点最大通道误差0（annotation-slab-union-pixels.log）。采集CPU透明合成完整网格证据与该原生抽样分开记录；后者不是完整App画质或物理显示。最终67660 exit0：workspace495通过/47忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release1m10s、diff check通过，UI build.rs覆盖Slint。日志target/annotation-slab-union-{cost,pixels,workspace,clippy,release}.log。无运行中构建、不暂存/提交/推送，无新实际App截图。

冷裁剪瓶颈已取得本批对应证据，后续回到全目标：来源新鲜度/系统同步、剩余协调器草稿交接与CPU副本、窗口实机焦点/IME/跨屏、完整画质及资源释放。20ms系统间隔未达标、50ms真实输入到显示未测；当前实现和测试不能宣称整体重构完成。下一批建议处理annotation.rs中绘制完成/取消与draft.take交接，让已有绘制拥有者返回明确终止结果，维持单次撤销和迟到事件规则；GPU时序诊断继续作为独立批次。


## 单次绘制终止结果与草稿交接（2026-10-04）

UI内部drawing::Press新增消耗自身的release，返回明确Release：Add(Object)、StageMosaic(Object)、AwaitArrow(ArrowPending)、Magnifier{source,style}。箭头阈值/max_distance/合法终点和第二点等待由该拥有者决策；普通对象和笔刷在最终位置更新后take显示草稿；马赛克使用按下DPI并继续交给mosaic.stage验证，不能绕过首次采集失败规则；放大镜仅交付来源区域，删除松开时重建后立刻丢弃的预览对象。放大镜采样/计时与对象创建仍由既有协调器接线。当前参数投影来源、普通对象release viewport scale及Core add/撤销语义保持原规则。

annotation.rs松开入口不再解构PressInfo后重新判断/计算上述几何，而是从现有参数模块借用有效values，按Release执行跨模块交接。显示draft仍由Registry持有，移动preview与multi-click流程尚未全部迁移，未声称Registry职责已完全清除。Press.cancel负责清空其display draft；Active.cancel接受必要draft借用，Drawing分支交给该拥有者，其他动作/几何/文字/序号继续所属协议，不关闭不属于绘制的Core事务。协调器保留取消参数回放、Pending和全局显示清理。

首次编译失败505466：移除导入时遗漏Registry::arrow_pending返回类型和测试中的ArrowPending；恢复必要类型导入，纯arrow_drag_commits导入只保留cfg(test)。未用allow(dead_code)保留无用路径。

新增2项回归覆盖100%/125% scale、单点/连续笔迹最终端点及一次undo清空、箭头长拖回起点仍等待合法第二点、直接有效拖动提交、马赛克跨release scale2仍按press DPI且返回采集stage、放大镜来源bounds、取消草稿。现有active错类型completion/参数事务保护，以及真实Slint受控canvas切换工具/Escape/取消/迟到松开、polyline键盘完成、马赛克/放大镜/普通文字回归保持。定向76084 exit0：annotation筛选190通过/3忽略（annotation-drawing-release-tests-after.log）。受控事件不等于实际App的鼠标捕获/IME/跨屏验证。

最终37929 exit0：workspace497通过/47忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check通过，UI build.rs编译Slint。日志target/annotation-drawing-release-{workspace,clippy,release}.log。没有GPU/Slint视觉布局/配置/依赖/Core公开接口变更，无新实际App截图、不暂存/提交/推送。没有运行中构建。

下一批收拢Pending折线和箭头的完成/取消显示交接，覆盖double-click、Enter、Backspace最后节点、无效终点与迟到松开；复用既有Pending拥有者，避免另建并行状态。GPU新鲜度/系统同步、CPU剩余副本、窗口/IME/焦点历史/DPI/双屏/负坐标、完整画质/资源及20ms间隔/50ms真实输入显示仍保留。整体目标未完成。


## 多次点击等待与显示草稿交接（2026-10-04）

Pending新增place_polyline/finish_polyline/backspace_polyline/release_arrow/cancel，接收必要slot/draft借用，不接收Registry。place集中初始化、双击判定、节点追加与confirmed显示；Enter和双击共用finish；Backspace清除计时、保留确认节点或最后节点时退休等待/显示；取消同时清理slot和draft。Progress明确Retained/Retired，有效完成返回对象，最小节点不足仍按既有规则结束而不建对象。协调器只提交对象及退休PlacePoint捕获，保留迟到松开不重建规则。

release_arrow先验证slot类型；无效终点保留ArrowPending并清理本次点击预览，合法终点消费一次。finish/backspace/place折线遇到箭头类型返回None而不改原等待/草稿；箭头操作遇折线同样保护。这修补了一条错误类型place替换现有等待的旁路，正常用户路由不变。现有350ms/6DPI双击阈值、填充最小3节点/普通2节点、冻结参数、PointerLeft预览语义、Core add和撤销保持。Registry仍持有display draft，部分工具切换/会话退休直接清理、预览写入尚未全部收拢，未声称全架构完成。

新增pending_handoff回归覆盖初始化与confirmed显示、错误箭头操作保留折线、双击只含已确认节点、不重复完成、Backspace最后节点清理、错误折线操作保护箭头、无效终点保留、合法完成和空取消。原受控canvas键盘完成/取消/工具切换/迟到松开及minimum节点回归继续执行。初轮定向69503为190通过/3忽略，尚未包含之后补充的最终类型保护测试；最终workspace实际执行该新增测试。

首轮75199 workspace通过，Clippy exit101提示Progress大variant。Retired改为Option<Box<Object>>，仅有效完成分配一次，转交Session时取出对象，不clone轨迹；Retained/无对象退休不增加该分配。没有压制lint或新增依赖。最终14702 exit0：workspace498通过/47忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check通过，UI build.rs检查Slint。日志target/annotation-pending-handoff-{workspace,clippy,release}-accepted.log；首轮日志保留。不暂存/提交/推送，无运行中构建、GPU/视觉布局/配置/Core接口变化或新实际App截图。

下一批审计工具切换、会话关闭与Pending取消的直接清理入口，先识别是否属于同一操作，避免旧状态清理新拥有者。继续保留剩余草稿/参数/CPU副本、GPU来源新鲜度/系统同步、20ms间隔/50ms真实输入显示、真实焦点历史/IME/DPI/双屏/负坐标、完整画质和资源验收。全目标未完成。
