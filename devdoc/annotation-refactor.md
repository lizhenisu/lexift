## 2026-10-06：放大镜改为正向“包含标注”

- UI 新增放大镜专用 `include_annotations` 正向参数，默认勾选。创建与修改对象时取反映射到既有 Core `erase_annotations`，对象回填同样取反；其他工具共用的 `erase` 字段保持原义，不改 Core 或渲染算法。
- 主参数条、参数标题和备用设置入口统一显示 “Include annotations”／“包含标注”，同步全部 12 种语言目录。参数差异协议纳入新字段，沿用各工具设置保存、对象独立回填、连续新建及撤销流程。
- 实测发现英文新标签会被下一复选框遮挡，补充复用 CheckBox 的实际文字宽度测量；放大镜条按标签增加所需宽度，并同步后续控件位置、屏幕空间约束和换行高度。未改变其他工具的布局规则。
- 相关 UI 回归扩展默认创建、勾选／取消映射、已有对象回填、参数差异传递、后续创建继承及其他工具默认值；UI 296 passed / 10 ignored，UI 全目标 Clippy `-D warnings`、Windows workspace Release 构建（包含 Slint 编译）和 diff 检查通过。日志：`target/magnifier-include-ui-final.log`、`target/magnifier-include-clippy-final.log`、`target/magnifier-include-release-final.log`。
- 最终 Release App 在 125% 主屏和 100% 副屏检查默认包含形状、文字 T、序号 1；取消勾选通过像素比对确认排除三类标注；撤销恢复勾选及画面，撤销放大镜后原标注仍存在；切换工具后新建放大镜继承取消勾选设置。实际截图已核对英文标签完整、控件无重叠。脚本／日志：`target/magnifier-include-app-check.ps1`、`target/magnifier-include-app-final.log`；截图：`target/magnifier-include-{enabled,disabled,undo,inherited}-{100,125}.png`。全部语言目录编译验证，未逐种切换实机语言或修改用户语言设置。
- 未新增 Core 接口、配置或依赖；未暂存、提交或推送，保留此前用户改动。

## 2026-10-06：修复放大镜擦除缺口拖动闪烁

- 根因回归先复现失败：`GeometryDrag` 每次从按下快照重建外框，但 `Session::update_selected` 根据上一帧擦除列表是否相同决定恢复覆盖，使列表逐帧清空／重新出现。
- 新增领域 `ErasePart`，在原放大镜对象内分别保存输出画面／边框／阴影、采样框、连接线的覆盖。独立移动外框只平移输出缺口，采样框缺口保持原位，连接线重新生成并恢复完整；整体移动平移所有覆盖。重复擦除补齐缺失的组成部分，已存在的覆盖不重复记录。
- 共用几何写入采用 `replace_selected_geometry`，接受从按下快照计算的完整结果；复杂编辑的恢复规则也从同一基准计算，不再受上一帧预览状态影响。保持目标身份、编辑范围、单次撤销及取消恢复，不在移动热路径清理或重建 GPU 呈现。
- 软件与 GPU 场景使用同一分部裁剪：效果遮罩裁剪输出覆盖，源框、阴影、连接线和外框按原绘制顺序各自裁剪；复用装饰缓冲区及既有栅格缓存。命中区按对应部分检查缺口；覆盖归属参与签名、资产键和完整对象比较，旧场景不得命中新版本缓存。
- 新增 5 项回归，覆盖每次拖动更新、往返原点、复杂缩放恢复与取消、分部独立像素断言、GPU 场景遮罩／缓存失效，以及重复擦除撤销。矩形／椭圆、阴影、100%／125% DPI 和负坐标均由自动化覆盖。
- 最终 workspace：609 passed / 94 ignored；workspace Debug／Windows Release 构建（包含 Slint 编译）和全目标 Clippy `-D warnings` 通过。显式运行 Windows GPU 放大镜采样／alpha 遮罩及合成顺序测试，2 项通过。日志：`target/magnifier-erasure-{workspace-test,clippy,build,release,native-test}.log`。
- 实际 Release App 在 125% 主屏和 100% 副屏验证外框独立移动、整体移动、一次撤销及 Escape 恢复；拖动途中共 54 次缺口像素和 54 次边框像素检查通过，底层夹具同时由绿色变为蓝色。截图：`target/eraser-magnifier-hole-{before-drag,during-drag,after-drag,whole-move,cancel}-{100,125}.png`；脚本／日志：`target/magnifier-hole-app-check.ps1`、`target/magnifier-hole-app.log`。实机没有负坐标屏幕；真实外部应用强制抢走捕获、所有连接样式组合未逐项实测，沿用自动化取消协议与分部绘制回归。
- 未新增平台 Port、配置、依赖或 crate；未暂存、提交或推送。此前橡皮擦及其他用户改动保持原状。

## 2026-10-06：集中擦除流程与橡皮擦

- 新增 Core `annotation_erase` 领域类型：矩形、圆头轨迹与仿射覆盖；擦除记录属于原对象、水印或聚光灯共享背景，不产生可选中的擦除对象。工具创建／编辑范围统一由 UI `annotation_tools` 映射，铅笔的不可编辑规则保持不变。
- UI `annotation_erase` 负责冻结手势参数、连续采样和可见目标判定。仅对影响范围栅格做擦前／擦后比较，空白、空心形状内部及重复擦除不产生历史。矩形预览从事务基线重建，笔刷连接相邻采样点；一次事务统一提交或取消。
- 普通对象、文字／序号说明与连接线、水印、聚光灯背景、马赛克／模糊／放大镜的遮罩和装饰均应用共用矢量覆盖裁剪。软件绘制使用独立目标层后合成，避免清除其他目标或底层屏幕；GPU 场景携带相同裁剪栅格，覆盖状态参与内容、几何和效果版本检查。
- 整体移动、非等比缩放及文字旋转变换缺口；复杂参数编辑恢复原对象被擦部分并与编辑合并撤销。水印重新应用、聚光灯新建／编辑恢复对应共享覆盖。`clear_all` 是可撤销文档操作，恢复完整内容及下一序号；会话销毁的硬清空保持独立。
- 二级条提供矩形／笔刷 SVG、笔刷大小及全部清除。大小弹层复用共用滚动和工具条避让，笔刷滚轮逐格调整 1，范围 1–100、默认 12；半透明实际尺寸圆形光标只更新原生交互，不在悬停时修改文档。补充 12 种界面语言文案。
- 原生实测发现连续擦除／撤销后移动可能呈现异常颜色，CPU 对象参数、栅格及 GPU 合成缓冲均保持正确。撤销涉及擦除覆盖时，现在退役已接受的原生呈现及旧候选，复用现有 Port 的清理和版本保护重新提交文档；不在绘制热路径重建原生窗口。完整操作序列重新通过严格原色像素断言；实验性的交换链修改与诊断 I/O 均已撤回。
- 自动化：workspace 604 passed / 94 ignored；全目标 Clippy `-D warnings`、workspace Debug／Windows Release 构建、Slint 编译通过。另显式运行原生 GPU 效果、放大镜与真实桌面预乘 alpha 呈现测试，4 项通过。Core／UI 擦除回归覆盖负坐标、两种 DPI、四向矩形、快速笔刷、单点、全部对象类型、重叠与新对象、空白／重复擦除、命中、变换、复杂编辑回退、撤销／取消、清除和编号恢复、软件／GPU 遮罩及原生呈现失效。日志保存在 `target/eraser-{workspace-test,clippy,release,native-present}.log`。
- 实际 App：100%／125% 双屏通过矩形／笔刷／大小弹层避让、撤销、全部清除及恢复、Escape、移动缺口、跨工具条捕获与松开；马赛克、模糊、放大镜、聚光灯通过擦除后原屏幕像素比对、底层颜色变化及撤销恢复。脚本 `target/eraser-app-check.ps1`、`target/eraser-effects-app-check.ps1` 仅操作自有 App 和夹具，并在结束时清理自有进程。
- 截图位于 `target/eraser-{rectangle,brush-dot,brush-stroke,brush-size,clear,restored,cancel,move-hole,cross-toolbar}-{100,125}.png` 及实时效果对照截图。实机没有负坐标显示器；负坐标由自动化覆盖。真实外部应用强制抢走捕获及全部复杂文本旋转组合未逐项实机覆盖，取消与变换协议由自动化验证。
- 未新增 Port、配置、依赖或 crate；未暂存、提交或推送，`Eraser/` 用户参考图保持原状。

## 2026-10-06：统一标注菜单与选色器避让

- UI 的共用物理坐标定位器接入分组菜单和全部三级弹层，移除文字样式专用的自由条带定位。输入锚点、完整期望尺寸、工作区和工具条实际矩形，枚举无交叠区域；优先完整容纳、远离一级工具条的一侧、另一侧、左右及锚点距离，不足时选择最大可见面积的滚动视口。实际障碍分别裁入当前工作区，不合并成跨屏巨型矩形。
- 工具分组只避让主工具条；字体、线型、箭头端点、连接样式、水印排列／时间、序号外形／格式／说明大小、马赛克效果和各颜色目标统一避让两级工具条。候选宽度包括按钮宽度；不在选择位置后再次钳制到障碍区。无可用空间关闭失败实例并记录诊断，保留原有实例和目标校验。
- 分组菜单增加垂直滚动；颜色及文字样式保持完整内容尺寸，视口缩小时允许横／纵滚动。字体列表保留选中行定位，颜色拖动与 HEX 焦点、父面板返回、二级面板持续展开均沿用原有路径。移动工具条继续收起旧弹层，重开读取新边界。
- 纯函数矩阵覆盖常用全部尺寸、宽按钮、上下布局、四边、左右回退、空闲间隙、滚动裁剪、完全无空间、100%／125% DPI、负坐标及其他屏幕障碍排除。菜单构造回归覆盖所有 field 入口。受限选色器 220×120 的事件回归验证滚动后仍能点击 HEX；原有 FIFO、焦点和旧回调回归通过。
- UI 284 passed / 10 ignored，UI Clippy 全目标 `-D warnings`、Windows Release 构建（包含 Slint）及 diff 检查通过。测试夹具补齐真实显示流程所需的工作区；MinimalSoftwareWindow 不实现原生位置，位置断言由纯定位矩阵和实际 HWND 检查承担。日志：`target/popup-avoid-{ui,placement,scroll,clippy,release,diff}.log`。
- 实际 Release App 在 100%／125% 双屏检查分组、几何线型／选色器、箭头线型／两端、放大镜连接、文字字体／选色器、序号外形／格式／说明颜色／字体／大小、马赛克效果、水印排列／时间、屏幕底部字体／选色器。所有菜单原生矩形与应避让工具条均不相交，屏幕边缘向上展开，截图视觉核对正常。另跑选色器／背景面板原生焦点回归，验证打开时继续输入、HEX Enter 后正文输入及返回父面板。
- 脚本／日志：`target/popup-avoid-app-check.ps1`、`target/popup-avoid-app.log`、`target/popup-avoid-focus-app.log`；截图：`target/popup-avoid-*-{100,125}.png`。极小工作区和负坐标由自动化覆盖，未改变实机显示器配置。仅操作并清理自有 App／夹具，不新增 Core 接口、平台 Port、配置或依赖；未暂存、提交或推送。
## 2026-10-06：平铺水印改为参考图的 -45° 斜向点阵

- 平铺水印由屏幕轴对齐的正交网格改为参考图的斜向点阵：文字仍按 -45° 旋转，格点两个基向量分别沿 +45° 与 -45°，也就是行方向与阅读方向一致，因此相邻行沿对角线错开，形成参考图的条纹感。非平铺的七种定位、透明度、聚光灯共存、缓存与撤销路径不变。
- 参数没有沿用计划里的估计值，而是对参考图实测反推。词块主轴经字形偏差校正后为 -45°：把本实现的旋转设为 -45° 后，同一 PCA 测量得到 -49.09°，参考图为 -48.90°。格点基向量为 (58, 58) 与 (87, -87)，按该向量平移的墨迹重合度为 0.93／0.90；两个方向的间距都等于字形高 + 0.885 × 字形高。
- 间距改用栅格化水印的实际墨迹包围盒计算，而不是包含升降部的整行高。后者会让同样字号下的间距偏大约 1.7 倍。墨迹包围盒在每次背景重建时扫描一次小位图，不进入逐帧路径。
- 覆盖范围按整张位图的旋转包围盒外扩，再映射到格点索引空间取整，保证视口四角与边缘的半个字形也被绘制；格点锚定视口原点，重建时图案不会爬行。新增有限值／非正尺寸守卫，并设 25,000 个瓦片的上限：超过时等比放宽间距，使极小字号配超大屏幕时迭代次数仍有界。
- 新增几何回归断言参考图 85.3 × 43.2 墨迹盒下的列距 123、行距 82、两个基向量分别为 ±45°、列步进沿阅读方向、行步进与其垂直、两轴间距同为 0.885 × 字形高。另一项断言覆盖四边、限制索引空间外扩、验证结果确定，并覆盖零与 NaN 尺寸。
- 受忽略的导出测试 `dump_tiled_watermark_for_reference_comparison` 通过 `LEXIFT_ANNOTATION_RENDER_DIR` 输出 1490 × 818 平铺位图；本次并排比对图见 `target/tile-side-by-side.png`（上：参考图，下：本实现），角度、交错与疏密一致。
- 门禁：`cargo test -p lexift-ui --lib` 283 passed / 10 ignored；workspace 591 passed / 94 ignored / 0 failed；`cargo clippy --workspace --all-targets -- -D warnings` 无警告；`cargo build --release -p lexift-app` 通过。日志：`target/tile-{ui,workspace,clippy,release}.log`。
- 实际 Release App 在 100%／125% 双屏把水印「排列」保持 Tile、字号 22 截图：`target/tile-watermark-{100,125}.png`，可见 -45° 斜向点阵。脚本 `target/tile-watermark-app-check.ps1` 只操作自有 App 与夹具，结束后清理自有进程。
- 参考图实测脚本保留在 `target/{measure_tile,measure_words,fit_lattice,compare_tile}.py`（target 已被忽略）。本次不新增 Core 接口、配置、依赖或 crate，界面与翻译未改；未暂存、提交或推送。
## 2026-10-06：聚光灯与水印共存

- 修复共用背景绘制先画水印、再用聚光灯遮罩整体替换缓冲区的问题。当前合成顺序为聚光灯遮罩、水印、普通标注；水印在开孔和暗区都使用自身色值与透明度。
- 软件绘制与缓存 GPU 场景共用原有背景合成函数，无新增逐帧排版。显示器布局、多个开孔合并、圆角和草稿预览保留。
- 独立像素回归以无聚光灯的水印字形作为基准，在黑色遮罩上验证全部字形 RGB 完全一致，覆盖开孔、暗区、边缘、矩形／椭圆、35%／100% 水印透明度、100%／125% DPI 和负坐标。旧的整体缓冲区替换会使这些非零 RGB 断言失败。
- 缓存回归检查两种创建顺序、多个聚光灯、移动／缩放、草稿取消、删除、撤销和手势取消；保留软件与场景合成一致性及复用缓存断言，并新增独立的水印色值存在性断言。
- UI 测试 281 passed / 9 ignored；UI 全目标 Clippy `-D warnings` 与 Windows Release 构建通过。日志：`target/spotlight-watermark-{ui,clippy,release}.log`。
- 实际 Release App 在 100%／125% 双屏验证平铺水印与矩形、椭圆、多个聚光灯共同显示，屏幕像素检测确认红色水印持续存在。脚本仅操作自有 App 与夹具并清理自有进程。实机没有负坐标显示器，相关坐标由自动化覆盖。
- 实测脚本／日志：`target/spotlight-watermark-app-check.ps1`、`target/spotlight-watermark-app.log`；截图：`target/spotlight-watermark-{rectangle,ellipse,multiple}-full-{100,125}.png`。本次不新增 Core 接口、配置或依赖；未暂存、提交或推送。
## 2026-10-06：自定义选色器仅在点击 HEX 时接收文字输入

- 当前编辑目标的正文、描边和背景选色器允许原文字编辑实例保持输入焦点。显示完成及色板拖动结束复用现有实例校验、前台权限和光标重置；水印独立窗口规则保持不变。
- HEX 输入默认不启用，通过字段上的显式点击启用并聚焦，避免原生窗口激活自动进入 HEX 编辑。直接输入期间阻止正文焦点恢复；有效色值 Enter 确认后退出字段编辑并返回正文，无效值保留输入焦点与错误提示。字段失焦或开始拖色结束 HEX 输入状态，拖动期间保持捕获。
- 回归覆盖颜色弹层的焦点放行、HEX 输入保护、有效／无效确认、拖动保护及草稿／插入位置／选区保留。UI 279 passed / 9 ignored，UI Clippy 全目标 `-D warnings` 和 Windows Release 构建通过；构建同时验证 Slint 编译。
- 实际 Release App 在 100%／125% 双屏验证打开选色器直接输入正文 P、点选颜色后输入 Q、点击 HEX 输入 123456、Enter 后输入正文 R，以及 Escape 返回背景面板后继续输入。原生前台 HWND 断言通过，截图可见正文保留连续 PQR，选色器仍打开且颜色为 #123456。
- 脚本／日志：`target/color-picker-focus-app-check.ps1`、`target/color-picker-focus-app.log`、`target/color-picker-focus-{tests,clippy}.log`；截图：`target/color-picker-focus-{open,hex,confirm}-{100,125}.png`。仅操作自有 App 和夹具并清理自有进程；未暂存、提交或推送。真实拼音组字未额外实测。
## 2026-10-06：文字背景设置的有序开关、持续输入与避让

- 背景和描边按钮改为 UI 内部单个有序请求：校验当前面板实例／工具／编辑目标，退休旧弹层，应用开关参数，再按最新目标创建新弹层。第一次点击启用并打开，第二次关闭功能与弹层；外部点击和 Escape 只收起弹层。
- 只有仍匹配当前编辑目标的文字样式面板允许正文焦点恢复，数值输入和拖动期间阻止恢复。展示完成、预设色点击、数值确认和滑杆结束恢复原编辑实例并重置光标，不重新创建编辑器或改变选区。预设色和滑杆操作明确结束 Slint 自动获得的数值输入焦点。
- 返回当前文字编辑窗口的原生焦点变化保留面板，并重新启用关闭监控；其他应用的焦点变化仍关闭。所有延迟操作继续校验会话、工具条、菜单和文字编辑实例，保留原有前台权限检查，不跨应用抢焦点。
- 文字输入窗口的键盘 Escape 单独接线：先关闭菜单链，保持编辑实例及输入队列有效；没有弹层时按原语义取消文字编辑。窗口关闭、明确取消和完成仍使用原有结束路径。共用流程覆盖序号说明。
- 样式面板和其自定义选色器使用两级工具条的物理边界，优先向远离一级工具条的一侧展开，屏幕边缘尝试其他自由区域。狭小空间限制窗口高度，通过 ScrollView 保持设置可访问；普通字体等菜单保留既有定位和焦点规则。
- 新回归覆盖开关、连续重开、旧回调失效、正文焦点放行、数值控件忙碌保护、Escape 后草稿／选区／编辑队列保留、预设色结束数值输入，以及 100%／125% DPI 的负坐标和屏幕边缘避让。已有序号、描边、自定义选色器、文字排版、组字和撤销测试通过。
- workspace 587 passed / 93 ignored；Clippy 全目标 `-D warnings`、workspace 构建、独立 Slint 编译及 Windows Release 构建验证日志：`target/text-background-{workspace,clippy,build,slint,release}.log`。
- 实际 App 在 100%／125% 双屏验证首次点击即展开、面板打开时直接输入中文、圆角输入 8 并 Enter、滑杆调整、再次点击关闭、Escape 后继续输入、自定义选色器返回以及工具条靠近屏幕底部的向上展开。原生 HWND 焦点断言与文字窗口尺寸增长检查均通过；截图核对连续正文 `abc中文DEFGHIJKLMX`、控件值及两级工具条避让。
- 复现脚本和日志：`target/text-background-app-check.ps1`、`target/text-background-app.log`；截图：`target/text-background-{first,open,adjusted,disabled,escape,custom-return,edge}-{100,125}.png`。脚本仅操作本次启动的 App 和自有夹具，结束后清理自有进程。
- 限制：实机没有负坐标显示器；中文实测采用原生 Unicode 键盘事件，真实拼音候选／组字未逐场景实测，也未额外改变操作系统输入法设置。相关自动化组字回归仍通过。不新增 Core 公开接口、平台 Port、配置或依赖；未暂存、提交或推送。

## 2026-10-05：统一后续绘制设置与手势快照

- 此项替代下文荧光笔的形态专用补丁。当前产品规则为“面板显示什么就画什么”，包含颜色、线宽、字体及全部样式；各工具保持独立，矩形／椭圆共享几何工具设置。
- `annotation_parameters::State::remember_creation` 只保存绘制参数，剔除正文与说明内容；对象参数回填不写默认值。接受对象参数修改时同步后续设置；确认创建时在清除选择前统一捕获当前显示值。序号编号继续由会话计数器决定。
- `drawing::Press` 持有完整参数副本，移动／松开不再从面板读取；放大镜完成返回该副本。箭头待完成状态和折线首次按下构造的对象保持首次样式，后续节点不重新初始化。普通文字按下也持有副本，松开创建编辑实例时沿用实际字体。
- 取消草稿不回退已确认设置；语言默认字体仅在字体目录初始化、语言变更和会话重置时更新，不在清除选择时覆盖继承字体。对象回填不标记为手动字体选择。水印继续使用原有独立规则。
- 新增 UI 回归覆盖全部目标工具的对象回填→空白按下→清除选择→取消，包含两种 DPI 和负坐标；另覆盖箭头两次点击、折线多节点和放大镜的冻结参数。更新旧策略断言，保留对象内容、旧对象样式、编号、目标实例校验、撤销及取消检查。
- workspace 585 passed / 93 ignored；workspace 构建、Clippy 全目标 `-D warnings`、独立 Slint 编译及 Windows Release 构建通过。日志：`target/creation-settings-{workspace,build,clippy,slint,release}.log`。
- 实机自有 App 与夹具在 100%／125% 双屏检查：荧光笔矩形连续绘制及返回线段；已有箭头改虚线／线宽后创建第二条；已有折线改曲线／线宽后创建第二条；已有放大镜改椭圆／边框／倍率至 6.1×／连接样式后创建第二个。面板保持展开，截图已核对新旧对象形态与显示参数。驱动早期误把分组坐标点到水印，已按实际窗口布局修正。
- 复现脚本与日志：`target/creation-settings-app-check.ps1`、`target/creation-settings-app.log`；截图：`target/creation-settings-{arrow,polyline,magnifier}-{before,after}-{100,125}.png` 和 `target/creation-settings-highlighter-{rectangle,line}-{100,125}.png`。
- 负坐标没有实机显示器，由 UI 回归覆盖；本次没有逐工具实测中文输入法组合和屏幕间绘制迁移。其既有自动化测试仍通过。未新增 Core 公开接口、平台 Port、配置或依赖；未暂存、提交或推送。

## 2026-10-05：荧光笔矩形开始绘制时切回线段

- 失败回归先复现：已有荧光笔对象选中并切为矩形后，空白画布按下触发 deselect，shape 从 1 变为默认 0。日志：`target/highlighter-mode-before.log`。
- 用户明确切换荧光笔形态时，仅同步后续默认 shape；新绘制按下在清除对象投影前保存面板当前 shape。对象颜色、线宽等参数仍与默认值隔离，其他工具规则不变。
- UI 回归覆盖显式线段→矩形和直接选择已有矩形两种入口、连续绘制、取消、撤销、切换工具返回、手动切回线段；100% / 125% DPI 与负桌面坐标均通过。旧对象形态转换及撤销行为保留。
- workspace 583 passed / 93 ignored；workspace 构建、Clippy 全目标 `-D warnings`、独立 Slint 编译、Windows Release 构建通过。日志：`target/highlighter-mode-{workspace,build,clippy,slint,release}.log`。
- 实际 App 100% / 125% 双屏：选中已有线段切为矩形，再连续绘制两次；用远离对角线的内部像素验证区域覆盖，手动切回线段后对应位置保持底图。截图已核对按钮选中态和真实绘制形态。首次测试驱动错误地假定默认红色，已改为按实际默认绿色计算 RGB 差异，非应用失败。
- 复现脚本／日志：`target/highlighter-mode-app-check.ps1`、`target/highlighter-mode-app.log`；截图：`target/highlighter-mode-{rectangle,line}-{100,125}.png`。仅操作自有 App 和夹具，结束后清理进程；负坐标无实机屏幕。
- 未新增 Core 接口、配置或依赖；未暂存、提交或推送。

## 2026-10-05：按当前工具条目限制对象编辑

- Core `EditScope` 是会话交互状态，不进入对象或撤销快照。UI 显式设置当前范围；其他 Core 使用者默认保留原有通用编辑行为。
- 几何、聚光灯、荧光笔、马赛克各自包含同条目内的两种形态；箭头、折线、放大镜、文字、序号分别独立；铅笔始终不可编辑。鼠标穿透、水印和清空条目不提供对象编辑范围。
- 命中在查找控制点和从上至下查找对象前过滤范围，异类上层对象不会阻挡当前范围对象。选中参数更新、删除、撤销恢复、失败马赛克清理及延迟几何写入也校验权限。
- 切换条目先按现有协议结束编辑／参数事务和取消手势，再清除不兼容选择、悬停和参数绑定。切换立即重绘选框并更新光标；关闭会话先清除悬停再同步范围，避免重新排队刷新。
- 取消点击对象和撤销时的跨条目自动切换。切回原条目需要重新点击对象；同范围内形态切换保持选择。延迟参数请求仍校验原对象和当前条目，不转写新对象或默认值；保留序号放置期间已接受的默认参数回放。
- 增加范围矩阵、异类重叠过滤、撤销／清空范围保留、过期拖动写入、光标及工具切换参数测试。旧测试的对象编辑场景显式进入对应工具，参数撤销测试先在对应条目接受参数再切换。
- workspace：582 passed / 93 ignored；workspace 构建、Windows Release 构建、Clippy 全目标 `-D warnings` 和独立 Slint 编译均通过。最终 Release 构建日志为 `target/annotation-edit-scope-release-final.log`。日志位于 `target/annotation-edit-scope-{workspace,clippy,slint}.log`。
- 实际 App：100% / 125% 双屏通过矩形绘制→切换序号→在矩形边线上按住放置序号→切回几何移动矩形的流程。像素断言确认异类对象保持原位；截图核对旧选框在切换后消失，二级面板保持显示。脚本只操作本次启动的 App 与自有夹具，结束后销毁会话并停止自有进程。
- 截图：`target/annotation-edit-scope-{selected,switched,returned}-{100,125}.png`；实机日志与复现脚本：`target/annotation-edit-scope-app.log`、`target/annotation-edit-scope-app-check.ps1`。
- 负坐标由 Core／UI 回归覆盖，实际桌面没有负坐标显示器。本次没有逐工具手动输入法组合测试；既有说明、文字、连接箭头、参数连续调整及捕获取消测试通过。
- 未新增配置、依赖、平台 Port；未暂存、提交或推送。旧 GPU 性能／资源重构工作不由本次验证标记完成。

# 标注模块与 GPU 效果链路重构

## 目标与边界

按已确认计划覆盖全部标注工具；分阶段替换，允许近似模糊，完成 Windows GPU 采集、合成和显示，以常用区域与跨屏操作的 60FPS 为目标。保留文字排版、中文输入、焦点策略、撤销和二级工具条行为。

七个 crate 和依赖方向保持不变：Core 管文档、编辑事务及能力接口；UI 管输入、窗口、编辑和不可变场景；Platform 管原生窗口、GPU 和采集资源；App 注入适配器。没有新增持久化配置或 crate，不暂存、提交或推送。

## 分阶段验收

|阶段|必须完成的内容|当前证据／状态|
|---|---|---|
|行为基线|全部工具操作、文字/说明/水印输入、取消、一次操作一次撤销|已有测试为基础，仍需整理原生回归场景|
|对象身份和事务|所有对象稳定身份，撤销/取消/清空不复用；共用编辑事务|已接入 ObjectId；重复开始不覆盖原事务；删除归入事务；连续参数更新不重复克隆文档。UI入口迁移仍待实施|
|UI 状态边界|会话、窗口、手势、文字编辑、参数、弹层、焦点分别持有状态|未完成；不得只搬文件而保留整 Registry 依赖|
|不可变场景|不含撤销历史的场景、局部图层、版本、依赖及增量失效|Core契约已定义；UI普通对象局部RasterLayer缓存已接入软件路径，背景/控制点共用绘制已分离；完整效果场景编译与依赖失效仍未完成|
|GPU 后端|WGC 清洁纹理→D3D11 效果/合成→DirectComposition 显示；正常路径不读回屏幕位图|普通RGBA、局部马赛克/模糊、放大镜及图层顺序合成通过真实GPU/桌面测试。App/UI接线未完成，实际标注仍走软件路径|
|算法|马赛克网格平均；小半径高斯、大半径 Dual Kawase，连续过渡及核半径校准|GPU马赛克和模糊已接入；半径/全档单调性/负坐标分块一致性GPU测试通过。完整UI画质回归仍待实施|
|调度与降级|最高60Hz、最新任务、静止不提交、设备故障与软件回退|未完成；当前34ms限制仍存在|
|最终验收|画质、性能、资源、构建与文档|未完成|

## 性能与画质验收

- 基准：当前 RTX 4060、Release、60Hz变化源；单个1280×720最大强度模糊、80逻辑像素长笔刷、四个重叠效果对象分别预热后测量30秒。
- 每场景平均显示更新≥57FPS、帧间隔P95≤20ms、GPU处理P95≤12ms、输入到显示P95≤50ms。采集、传输、处理、合成及显示独立计时；不以单项滤波基准代替端到端证据。
- 普通标注维持现有字形布局；马赛克与参考每通道偏差≤1；模糊有效σ偏差≤10%、强度单调，分块与整片同算法无可见接缝。
- 回归100%/125%、跨屏及软件负坐标；实际不可用物理场景如实记录。GPU故障模拟和快速关闭重开确认资源及旧回调清理。
- 最终运行workspace测试/构建、Slint编译、Clippy -D warnings和Windows Release；保存截图、统计及验收记录。软件降级不承担60FPS指标。

## 当前迁移说明

ObjectId 为会话内单调身份，0仅表示未插入草稿。Session::add 总是分配新身份，复制已插入对象也不能复用身份。身份随对象快照恢复，但分配器不随撤销、取消或清空倒退。参数更新不能替换身份。

马赛克现有 id 暂时镜像 ObjectId，以保持当前采集确认/失败回滚链路；待场景迁移完成后删除重复身份。对象数组次序继续表示文档绘制顺序，异步与缓存查找将迁移为 ObjectId，而不是依赖长期保存的数组下标。

选色参数现已迁入独立纯函数模块 annotation_parameters：正文、描边、背景和序号文字使用明确 ColorTarget，负数协议只在边界转换。参数测试覆盖四个目标互不覆盖及其他参数变化后自定义颜色保持。

Core渲染契约使用不可变Arc场景及RGBA图层；没有撤销快照、窗口组件或原生纹理。渲染完成状态返回场景版本、有效采样区域和是否已显示，供UI判断待确认对象。Platform已实现部分GPU适配器，但App/UI场景接线仍未完成，不能据此宣称现有标注已切换GPU或60FPS已完成。

## GPU显示原型与帧生命周期

- Platform新增独立GPU工作线程，普通图层按RasterKey上传，并在非激活的DirectComposition窗口显示；窗口位于所属Slint输入画布下方，透明区域和鼠标输入继续由原机制处理。
- WGC采集拆分为GPU纹理更新和原有软件读回。GPU接口不创建staging纹理或Map屏幕像素；这不意味着现有标注已切换为GPU。
- DXGI最大队列深度为1。在绘制前以零超时检查帧延迟句柄，多屏保留已经获取的许可；忙时留给下一轮处理，不阻塞会话关闭。句柄与窗口随Surface销毁，初始化失败也释放句柄。
- 跨会话提交会销毁上一会话的Renderer；即使RasterKey数值相同，也不能复用上一会话的纹理。
- WGC每轮最多读取帧池容量（2帧），保留最新帧并及时关闭被替代的lease；不无限追逐持续生产的帧。
- GPU工作线程按相邻帧开始时间限制最高60Hz，处理耗时计入同一帧预算；不在处理结束后额外等待完整16.67ms。此调度修正本身不证明60FPS指标达成。
- 真实桌面测试覆盖预乘RGBA颜色、半透明交互层混合、跨会话相同键的新像素、连续8次更新及关闭后GPU窗口清零。此测试没有证明效果合成、GPU资源总量归零或60FPS达标。
- 本轮Platform单元测试75 passed / 19 ignored，Platform/all-targets Clippy -D warnings通过。完整workspace回归另行记录；尚未运行本阶段Windows Release验收。

本轮最终回归：workspace测试376 passed / 22 ignored（target/annotation-refactor-current-workspace.log），workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-current-clippy.log），真实桌面GPU显示测试通过（target/annotation-gpu-display-native.log）。UI构建包含Slint编译。尚未接入效果或执行本阶段Release性能验收。

## GPU马赛克与有序合成阶段

- MosaicLayer传递对象元数据与可缓存的几何覆盖图层，不传递桌面像素。覆盖图层按RasterKey上传；效果瓦片缓存source/grid/filtered/output四张设备纹理，参数或几何边界变化时重建。
- 两遍马赛克：第一遍只对每个桌面锚定的网格计算一次平均值，第二遍用整数Load展开。负坐标采用Euclidean分区，避免网格随对象或屏幕原点移动。
- 采集请求局限于每个遮罩瓦片覆盖的完整网格；WGC纹理在GPU上合成，叠加该效果下方的普通标注及先前效果，处理后乘以覆盖alpha。上方标注、光标和控制点随后绘制，不进入采样。
- 相同场景且WGC没有新帧/几何变化时跳过效果计算与显示提交。这尚未覆盖WGC提供相同像素新帧的情况；GPU像素变化检测仍待实施。
- 已有有效瓦片与暂停区域相交时保留上一帧，暂停解除后重算。改变遮罩身份/几何时的暂停衔接，以及所有弹层流程，仍须在UI迁移后验证。
- GPU原生测试四项通过：预乘显示/清理；网格平均与遮罩；下层标注采样及上层清晰；WGC→局部马赛克→DirectComposition实际桌面显示、暂停/恢复及关闭清理。
- 网格对照覆盖1/2/3/16/64/80物理像素、负坐标和非网格对齐的输入边缘，每通道误差≤1；遮罩覆盖0/64/128/255 alpha。GPU输出仅在测试中readback，生产效果路径不Map像素。
- 实际桌面截图保存于target/annotation-gpu-pixelate-native.png及target/annotation-gpu-order-native.png，内容均为测试自身不透明图层；后者已经视觉检查。原生测试日志target/annotation-gpu-pixelate-native-tests.log。
- surface坐标范围在Core拒绝无法以i32表示宽高的场景；瓦片网格扩张做饱和对齐并检查尺寸减法，避免负坐标/极端输入导致panic。
- 本阶段最终workspace测试378 passed / 25 ignored（target/annotation-refactor-pixelate-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-pixelate-clippy.log）。四项GPU原生测试另行显式运行并通过；常规workspace测试不会自动执行它们。

## GPU模糊、校准与纹理复用阶段

- σ≤3物理像素使用两遍离散高斯；3<σ<4混合高斯与Dual Kawase结果；σ≥4使用最多5层的下采样/上采样。10–12、20–24、40–48物理像素范围内混合相邻层级，避免层数切换造成画面跳变。
- 高斯和金字塔中间纹理为RGBA16F，最后遮罩及显示阶段才写回8位目标；生产路径仍不读回桌面或效果像素。
- 校准计算线性插值在内的离散边缘核，并对金字塔的四个相位求方差、二分选取偏移。校准计划按σ缓存于Effects，非逐帧扫描图片；场景销毁时一起释放。
- 采集边缘按所有下采样、上采样及双线性插值的有限支撑求和；ROI锚定桌面的2^层数网格，保障负坐标和分块的相位一致。宽高/边缘膨胀做溢出保护，非法非有限参数在Core场景验证拒绝。
- 实际GPU脉冲验证覆盖σ0.5–60、高斯/算法过渡、层级过渡及相位变化，测量σ偏差≤10%，质量守恒误差<3%。100%/125%各100档强度均为单调增加。
- 实际GPU分块验证覆盖σ2、3.5、4、10.5、22、25，负坐标的两片ROI与整片参考每通道差异≤一个8位色阶；没有依靠位图扫描计算生产边界。
- 马赛克/模糊瓦片按稳定asset缓存，修订/位置变更后尺寸与滤波拓扑兼容时复用设备纹理；改变σ但保留层级时仅更新偏移。普通RGBA及遮罩上传也复用同asset、同尺寸的设备纹理。GPU测试检查资源身份与移动后像素，原生桌面测试检查修订后颜色更新。
- 六项GPU原生测试通过（target/annotation-gpu-blur-native-tests.log），其中真实桌面场景现在包含马赛克→模糊切换，并对阶跃输入的模糊结果与独立参考比较。实际模糊截图target/annotation-gpu-blur-native.png已经视觉检查。
- 核测量target/annotation-gpu-blur-kernels.csv，强度测量target/annotation-gpu-blur-strengths.csv；这些是画质验证，不是Release端到端性能报告，也不证明物理125%窗口交互已完成。
- 本阶段最终workspace测试379 passed / 27 ignored（target/annotation-refactor-blur-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-blur-clippy.log）。UI构建包含Slint编译；六项GPU测试另外显式运行，不计入常规自动测试的passed数。

下一实施点：完成UI局部场景构建、App注入、UI状态所有权拆分及故障降级。当前未执行完整Release性能验收，也未完成全部标注UI重构。

## GPU放大镜与采集停止阶段

- MagnifierLayer包含对象元数据、可见输出遮罩和独立装饰图层。源区域由可见输出反推，只请求局部源纹理，双线性模式增加边缘并限制在原源区域内。
- GPU支持最近邻与双线性放大。保留原有语义：普通标注（包括后绘制的普通标注）可进入放大镜，仅较早的马赛克效果进入取景；其他放大镜、边框及交互覆盖层不递归进入取景。Erase模式只显示清洁屏幕源。后续马赛克可以采样已经合成的放大镜输出。
- 遮罩、连接线、边框、阴影的最终生成仍需由UI共用绘制接线；本阶段原生测试使用显式测试图层，不代表实际工具已迁移。
- 八项显式GPU测试通过，日志target/annotation-gpu-lens-native-tests.log。新增最近邻/双线性/裁切输出的负坐标像素对照，以及Erase、非递归取景和与马赛克的叠放测试。真实桌面圆形放大镜截图target/annotation-gpu-lens-native.png已经视觉检查。
- 移除最后一个需要屏幕源的效果时，清空WGC会话与源纹理；保留D3D设备供普通标注显示。没有有效遮罩或强度为零的效果不持续触发60Hz调度。
- 原生测试从实时放大镜切换到纯普通图层，确认切换前采集实例非零、切换后归零、普通图层颜色正确，最终窗口清零。日志target/annotation-gpu-capture-lifecycle.log；实例计数仅用于测试。
- 本阶段最终workspace测试381 passed / 29 ignored（target/annotation-refactor-capture-lifecycle-workspace.log），workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-capture-lifecycle-clippy.log）。Slint编译随UI构建执行。关闭采集后的原生测试另行显式通过。
- 尚未接通实际应用GPU场景、设备故障回退或完整Release性能验收。

## 文档与交互覆盖层失效分离

- GPU效果输入缓存独立于RenderStamp：比较有序文档图层的RasterKey/边界、效果对象参数、遮罩、装饰和暂停区域。交互覆盖层、显示窗口和光标修订不属于效果内容；帧循环不逐字节比较栅格像素。
- RasterKey是会话内不可变像素身份，像素或尺寸变化必须提升revision；不改变像素的位置移动可以保留key。UI场景编译器必须遵守此约定，尚未完成该编译器接线。
- 新屏幕源变化始终触发效果更新，即使文档stamp不变。效果输入仅在成功计算后保留，失败可继续重试。
- GPU测试显式检查：首次计算红色源；只修改interaction/revision时拒绝重算并保留红色效果；源变化后同一文档更新为绿色。独立单元测试覆盖普通图层修订、遮罩位置、强度、图层顺序和暂停区域的失效。
- 九项显式GPU测试通过（target/annotation-gpu-overlay-native-tests.log），包括先前马赛克、模糊、放大镜、原生显示、采集停止，以及本轮覆盖层缓存测试。
- 最终workspace测试383 passed / 30 ignored（target/annotation-refactor-overlay-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-overlay-clippy.log）。这不证明实际应用的光标已避免CPU重绘，应用仍待迁移。
- 当前仍为文档级效果失效；按单个对象与采样依赖进一步缩小范围、GPU相同像素检测、UI场景编译及App注入继续待实现。

## UI局部内容缓存与共用绘制阶段

- 新增annotation_scene::SceneCompiler，编译普通标注为不可变局部RasterLayer。缓存按ObjectId与视口分槽，未插入草稿/测试对象使用独立临时槽；像素、样式、几何、字体参考或DPI变化时提升资产revision。
- 文字、序号、形状、笔迹、线条、箭头继续复用共用绘制函数。几何边界包含旋转、说明及连接箭头，并在分配前裁到可见视口，不扫描整张位图。水印/聚光灯属于视口背景缓存。
- 选框、操作点与悬停反馈抽取为共用draw_interaction，背景抽取为draw_backdrop；软件原路径与缓存路径调用同一份函数。光标/选区/组字覆盖仍由原编辑流程最后绘制，不进入内容缓存。
- 实际UI的无实时效果场景已经使用该缓存；选择/光标变化复用正文像素，单个对象变化只重新生成自身栅格。最终软件合成仍分配画布帧，不代表已经完成GPU默认路径或端到端60FPS。
- 删除对象、对象离开视口、视口移除及会话结束释放对应像素。缓存仅保留对象/绘制元数据，不保留Session及撤销栈。
- 大字号旋转文本的局部裁剪曾与旧全画布坐标产生边缘差异。共用绘制现在在桌面坐标按1/64物理像素误差展开旋转曲线，并对轮廓使用统一1/256子像素网格后整数平移；未旋转文字保留原曲线绘制。字体、字号、排版、输入位置计算没有改变。
- 四项新测试覆盖全部普通种类、选择/悬停、正文缓存身份及局部失效、水印/聚光灯、删除/视口释放，以及96/400字号、旋转/粗描边/背景、中文组字参考和负坐标视口接缝；覆盖100%/125%比例。缓存与共用完整绘制每通道差异≤1。
- 最终workspace测试387 passed / 30 ignored（target/annotation-refactor-scene-final-workspace.log），workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-scene-final-clippy.log）；UI构建包含Slint编译。
- 完整AnnotationScene提交与App渲染注入仍待实现。有实时效果的实际场景暂时保留原软件路径，后续迁移须保持文档顺序与现有放大镜规则。真实交互截图及完整Release性能验收仍待执行，不能用栅格测试替代实机证据。

## UI效果元数据、遮罩及装饰编译

- SceneCompiler::layers将所有视口的背景及普通对象、MosaicLayer和MagnifierLayer按文档顺序编译；普通软件路径继续消费同一套编译顺序。效果图层仅传递对象元数据与几何，不采集或保存桌面像素。
- 马赛克区域与长笔刷使用现有几何/分区规则生成局部遮罩；放大镜生成输出遮罩及共用边框、连接线和阴影。元数据按稳定对象身份复用Arc，未变化时不重复克隆轨迹。
- 遮罩缓存排除强度、滤波类型与不影响形状的样式字段；放大镜装饰缓存排除zoom/antialias/Erase等采样参数。修改这些参数仅更新对象元数据，保留几何RasterKey和像素Arc。
- 新增annotation_path共用轮廓与覆盖工具，承接原文字轮廓规范化及效果覆盖生成。覆盖使用桌面锚定256物理像素单元与4像素边缘，再裁到请求ROI。固定点扫描器在整片和分块调用中使用相同原点，避免翻译/裁剪产生覆盖偏差。
- 笔刷覆盖从实际points与直径计算活动边界，兼容草稿反馈先更新轨迹、后更新保存bounds的过程；已有单点笔刷与未完成反馈回归通过。
- 新测试验证普通/效果/上层普通标注的顺序、Core场景验证、100%/125%视口下的参数变更资产复用、删除清理；负坐标长笔刷分块与整片覆盖误差≤1；跨视口圆形放大镜遮罩与整片输出逐字节一致。
- 最终workspace测试389 passed / 30 ignored（target/annotation-refactor-effect-scene-workspace.log），workspace/all-targets Clippy -D warnings通过（target/annotation-refactor-effect-scene-clippy.log），包含Slint编译。
- 此阶段没有宣称实际应用已使用GPU，也未执行新Release/端到端性能验收。交互覆盖层的RasterLayer编译、完整AnnotationScene提交、App注入、软件降级与UI状态所有权拆分继续待实施。

## UI交互覆盖层编译与共享分块

- SceneCompiler将选框、操作点、悬停反馈和编辑光标/选区/组字下划线编译为独立的局部RasterLayer，缓存与正文分开。重复状态复用RasterKey及像素Arc；视口移除和会话关闭清理覆盖缓存。
- 普通文字与序号说明共用EditorOverlay；实际无实时效果场景已消费独立覆盖层，有实时效果的软件降级路径也调用相同编辑反馈绘制，避免保留两份选区/光标实现。
- 彩色覆盖与效果遮罩共用桌面256像素单元枚举、4像素保护边及裁剪规则。局部与整片绘制使用相同扫描原点，不分配全屏反馈临时位图；彩色合成与遮罩填色仍保留各自像素策略。
- 新测试覆盖旋转中英多行正文、光标显示/隐藏、选区、组字、空说明光标、100%/125%比例及负坐标；覆盖层变化不改变正文资产，重复调用复用像素，软件画面与共用参考每通道误差不超过1。
- workspace测试390 passed / 30 ignored（target/annotation-interaction-workspace.log）；UI七项场景对照测试通过（target/annotation-interaction-tests.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-interaction-clippy.log），包含Slint编译。
- 完整AnnotationScene提交、App GPU注入、故障恢复、UI状态所有权拆分和端到端Release性能验收仍未完成；本阶段没有实际GUI截图或GPU性能结论。

## 完整场景组装与多视口软件消费

- SceneCompiler::compile组装Core AnnotationScene：所有视口的有序文档、效果元数据与几何遮罩、最后绘制的交互覆盖、暂停区域和排除窗口。输入包含RenderStamp，不保存Session或撤销历史。
- 窗口标识通过WindowLifecycleCallbacks由App注入Platform查询，UI没有原生API或横向crate依赖。普通标注的实际软件显示已按整份场景合成；窗口尚未就绪或无原生标识时沿用局部软件路径。
- 在修改缓存前拒绝非法显示表面；编译时清理已移除视口。软件普通合成明确拒绝含效果的场景，避免把尚未接通的实时效果静默丢弃。
- 跨视口测试发现圆角形状在不同局部原点裁剪时有16级alpha差异；Rectangle/Ellipse/HighlightRectangle的共用绘制改用固定桌面单元，使局部资产与完整画面一致。字号、文字路径与排版未在本阶段修改。
- 新测试覆盖双视口100%/125%比例与负坐标、资产复用、交互隔离、非法窗口拒绝、移除及空视口清理。原效果顺序测试改为调用完整编译入口，验证马赛克/放大镜顺序及软件普通合成拒绝效果场景。
- workspace测试391 passed / 30 ignored（target/annotation-complete-scene-workspace.log）；场景对照八项通过；workspace/all-targets Clippy -D warnings通过（target/annotation-complete-scene-clippy.log），包含Slint编译。git diff --check通过。
- 当前仍未把AnnotationRenderPort提交接入实际UI；实时效果继续走既有软件/原生放大镜路径。原生完成反馈、透明输入画布交接、故障降级、首次采集失败回滚及Release性能/实际交互截图均待实现或验收。

## 普通标注GPU提交及软件交接

- App创建AnnotationRenderPort并注入UI生命周期；新增annotation_presenter::Presenter，独立拥有提交版本、完成版本、首帧/停滞等待、启用状态与轮询计时器，不访问Registry或保留对象/撤销历史。
- 普通标注场景已提交至GPU适配器。成功显示反馈到达前保留软件画面；反馈通过会话/版本校验后，输入窗口提交透明像素一次，后续场景由GPU显示。Slint输入、IME、光标事件仍使用原输入窗口。
- 提交、异步显示、三秒无进展或透明输入交接失败时清理原生场景并回到软件；本会话不继续自动重试，新会话允许重新启用。关闭会话停止轮询并清理适配器场景。
- 旧会话、未来版本和presented=false反馈不授权交接；持续输入可接受同会话较早的成功显示作为进展，但停滞的旧反馈不能无限刷新超时。
- 当前按迁移顺序只启用普通标注GPU显示。有马赛克/模糊/放大镜对象时暂时暂停该提交路径、恢复软件输入像素并沿用旧效果编排；这不是最终降级规则，后续必须接通全部效果及采样确认。
- 三项Presenter测试验证反馈过滤、进展/停滞和关闭恢复。UI软件窗口+模拟Port测试执行实际render_canvases/poll回调，确认首帧软件保留、一次透明交接、不重复原生画布提交，以及设备错误后软件像素完全恢复。
- workspace测试395 passed / 30 ignored（target/annotation-native-handoff-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-native-handoff-clippy.log），包含Slint编译；git diff --check通过。
- 九项显式原生GPU测试通过（target/annotation-native-handoff-gpu-tests.log），包含桌面呈现/关闭、像素平均、模糊校准、顺序、放大镜和缓存。UI交接测试使用模拟Port，尚未作为真实应用端到端交互/性能证据。
- 全部实时效果UI提交、首次采集确认/回滚、设备重建、提交前版本屏障、GPU相同像素检测、Release性能、真实应用截图及UI状态所有权拆分继续待完成。

## 全部实时效果GPU接线与采样确认

- render_canvases现在向已注入的GPU适配器提交包含普通标注、马赛克、模糊、放大镜及交互覆盖的完整场景。GPU正常路径不启动软件屏幕采集/滤波线程，也不初始化旧原生放大镜；软件编排保留为故障/不可用降级。
- Presenter保留提交场景中的稳定对象身份及必须完成的输出边界。只有当前版本完整覆盖的成功显示才能确认对应临时马赛克；后续尚未提交的对象不会被旧完成反馈确认。暂停区域随工具条/弹层位置变化重新提交；实时效果期间持续轮询以发现异步错误。
- 修复新建暂停块被报告为成功的问题：暂停块不处理未初始化纹理，completed_regions仅包含有效输出。Renderer在任一必要输出未初始化时不提交部分画面，保留上一完整显示。Core反馈明确区分Source、Paused和Presentation等待；正常暂停不触发停滞超时，旧版本暂停不能延长新请求等待。
- 暂停源ROI不进入采集请求；全部源暂停时释放WGC捕获，保留已处理效果。取消暂停后重新采集。新块/新位置在暂停期间尚无结果时保持等待，不把空输出当成可提交内容。
- 扩展UI交接测试，实际编译马赛克与放大镜场景：不完整反馈不确认对象；完整反馈确认对应身份并交接画布；GPU正常路径没有CPU屏幕请求。该测试使用模拟Port，不能替代实际应用GUI测试。
- 扩展原生GPU测试：新建暂停场景反馈Paused、没有已采样区域且未显示；恢复后处理；已有结果暂停保留；独立效果测试覆盖马赛克/放大镜初始化与就绪判断。九项显式GPU测试通过（target/annotation-native-effects-gpu-tests.log）。
- workspace测试397 passed / 30 ignored（target/annotation-native-effects-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-native-effects-clippy.log），包含Slint编译；git diff --check通过。
- 尚未完成设备重建、提交前版本屏障、GPU相同像素检测、逐对象采样依赖失效、UI状态所有权拆分、Release性能与真实GUI/DPI交互验收。GPU首次采样失败后的完整降级/回滚、设备故障期间最后有效效果的保留，以及本进程普通窗口的采集排除范围还需专项验证；当前WGC枚举仍排除整个本进程。
- 复核当前源码：visible_session以Session::new建立只含当前可见文档的预览，不复制撤销历史；Session也没有Clone实现。仍然每次复制全部当前对象/轨迹，后续需缩小这一开销。不能把场景缓存已完成误记为整个UI预览链路已避免文档复制。

## GPU提交前版本屏障

- Mailbox::owns统一检查清理代次、会话/修订与停止状态。GPU准备、采集和绘制在锁外完成；DXGI/DComp非阻塞提交之前重新授权，并仅在该短提交区间与submit/clear串行化，消除校验与原生提交之间的竞争窗口。
- 每个视口先完成后台绘制，随后在同一授权区间提交整组表面；过期结果不提交，也不发布完成状态。准备期间发生清理、更新或关闭时，旧后台缓冲区保留为未提交结果，后续场景可复用绘制资源和显示许可。
- 三项普通并发测试覆盖准备期间取消、同代次新版本替换、关闭拒绝、授权提交与清理的顺序。测试使用通道/屏障，不依赖睡眠碰运气。
- 原生测试实际绘制红色后台缓冲区，然后在提交边界清理其代次；屏幕保持此前蓝色，presented_stamp未改变，销毁后原生表面数量归零。截图target/annotation-gpu-cancelled-frame-native.png已查看，只覆盖本测试拥有的100x80区域，不属于实际应用GUI交互验收。
- workspace测试400 passed / 30 ignored（target/annotation-present-fence-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-present-fence-clippy.log）；九项显式GPU测试通过（target/annotation-present-fence-gpu-tests.log）；git diff --check通过。
- 这证明逻辑提交在当前代次授权下执行，不等同于测量系统合成器的物理显示延迟。实际应用关闭/重开、多显示器时序和Release性能仍待验收。设备恢复、相同像素检测、依赖失效及UI所有权拆分继续待完成。

## 菜单窗口所有权与可取消准备任务

- Registry 中分散的菜单窗口和实例编号迁入 choice::State。菜单显示重试和跨 DPI 定位使用该状态拥有的两个计时器，关闭时立即停止并提升实例版本；已有延迟选择/返回任务继续按会话与实例校验。
- Menu 持有自身关闭监控的清理回调，在销毁时关闭监控并隐藏窗口。显式关闭和直接销毁会话均走同一资源清理路径，不依赖 Registry 必须调用某个关闭函数。
- annotation_choice 的生产 super 通配导入已改为明确依赖。参数合并、菜单返回层级及编辑焦点恢复保持现有顺序。菜单编排仍读取 Registry，完整的结果回调接口和窗口/焦点所有权迁移尚未完成，不能将本批记为 UI 职责拆分全部完成。
- 新生命周期测试验证关闭停止两类计时器、窗口释放、监控清理，以及绕过显式关闭直接销毁所有者。现有菜单定位、键盘导航、颜色目标及参数合并测试保持通过。
- 当前 workspace 测试 403 passed / 30 ignored，日志 target/annotation-menu-owner-workspace.log；workspace/all-targets Clippy -D warnings 通过，日志 target/annotation-menu-owner-clippy.log；构建包含 Slint 编译，git diff --check 通过。本批没有真实应用截图或新的 Release 性能证据。
- 先前设备恢复实现已通过九项显式原生测试（target/annotation-device-recovery-gpu-tests.log）。同一清理代次/会话仅允许一次重建，修订和成功显示不重置预算；Recovering 反馈在已激活的 UI 路径也启动停滞期限。原生测试注入设备丢失分类错误并检查重建后的桌面画面，不代表真实驱动故障已实测。
- 完整目标继续待实施：窗口/字体/参数/编辑/手势独立所有权、逐对象效果依赖、相同屏幕像素检测、本进程普通窗口采集、故障时最后有效画面保留，以及实际应用 Release 性能与多屏交互验收。

## 字体目录与焦点恢复所有权

- annotation_fonts::Directory 统一拥有安装字体名称、语言别名、系统无衬线回退、加载状态与文字/水印/序号三类手动偏好。生产代码不依赖 Registry，也不使用父模块通配导入；工具编号只在 UI 协调边界转换为明确的字体目标。
- 字体目录只加载一次，包括空目录情形；默认解析使用缓存的名称、别名和回退。对象回填与当前草稿保护仍由会话协调器计算，目录不持有文档、窗口或编辑状态。手动选择保留精确名称，负数或越界索引不建立偏好。
- 序号参数更新直接借用安装字体名称切片，不再克隆整份字体目录。原语言候选、本地名称解析、菜单显示和排序规则保持不变。
- annotation_focus::Coordinator 独立拥有下一帧恢复计时器及带会话/编辑/工具条/菜单身份的请求。后来的请求替代旧请求；工具条关闭、编辑完成、初始化失败或所有者销毁时取消。恢复前仍执行原有活动编辑、弹层、滑杆和输入控件校验，再归还焦点并重置光标。
- 新测试覆盖三类偏好隔离、无效索引、手动英文名称跨语言保留；焦点计时器使用可控 Platform 时钟验证最新请求替代、取消和销毁，不依赖实际睡眠。既有语言/对象回填和序号参数输入选区回归保持通过。
- 当前 workspace 405 passed / 30 ignored（target/annotation-font-focus-owner-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-font-focus-owner-clippy.log）；包含 Slint 编译，git diff --check 通过。本批没有新的原生截图或 Release 性能测量。
- Registry 仍协调对象回填、窗口激活、文字和手势状态；参数/编辑/窗口整体迁移及 GPU 剩余验收继续待完成，不能将目录和定时器独立误记为全目标完成。

## 文字编辑身份与实例任务所有权

- 新增 annotation_editor::TextEdit，拥有编辑实例编号、稳定对象目标、草稿排版反馈、鼠标选区锚点、光标状态，以及光标/窗口准备计时器。模块不依赖 Registry，不保留 Session 或撤销历史；对象下标在使用时通过现有 Session::object_index 解析。
- 普通文字和序号说明的编辑目标改用 ObjectId。前方对象删除导致数组位置改变后仍编辑原对象；目标已删除时不绘制替代草稿、不提交替代对象，也不产生撤销。生产编辑入口同时接收会话编号与 ObjectId，失效会话、目标缺失或目标类型不符时不创建窗口。
- 初始显示准备和后续重试均由当前编辑实例计时器拥有，保持初次零延迟、重试16ms及光标500ms规则。停止或销毁实例取消准备与光标任务；回调仍校验会话和实例，避免旧任务修改新编辑。
- 稳定身份检查发现序号放置手势保留未入库草稿：Core 已分配的 ID 在移动/松手时被覆盖为 DRAFT。现在拖动快照取自已入库对象，原地点击及拖动后均保留有效身份；删除了一次多余草稿克隆。
- 新回归在100%/125%比例验证普通文字及序号说明：删除前方对象后仍写回原目标；目标删除后的提交不修改其他对象或新增撤销。放置回归增加最终对象身份校验。可控时钟验证编辑准备任务只保留最新请求、停止/销毁同时取消准备和光标计时。
- 当前 workspace 407 passed / 30 ignored（target/annotation-editor-owner-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-editor-owner-clippy.log）；包含 Slint 编译，git diff --check 通过。本批没有新的真实应用截图或 Release 性能证据。
- 原生编辑组件及文字事件协调仍在 Registry，窗口/编辑完整合并、其他手势长期身份迁移、参数状态所有权及 GPU 余项继续待完成；本批不证明完整 UI 或端到端验收完成。

## 对象手势稳定身份与文档命令衔接

- Gesture 数据移入 annotation_gesture，移动、缩放、序号放置/控制、文字连接箭头及文字操作点保存 ObjectId，不再保存对象数组下标。软件马赛克移动预览、处理上下文及待验证对象取消也通过当前文档解析同一身份。
- 所有对象手势更新使用统一 guarded write：检查目标存在、快照身份/类型一致且目标可编辑，再调用 Core Session::update_selected。该函数用于已开始的拖动事务，保持选中状态、Core 身份维护及一次拖动一次撤销；铅笔不可编辑规则仍由 Core 判定。
- 删除了 Move/Resize 从未参与计算的 initial Bounds 及对应占位读取；实际几何起点继续来自原对象快照，没有改变移动/缩放公式或 DPI 规则。
- Undo、删除及工具切换前取消正在进行的对象手势并清理快照，避免 Core 事务结束后继续应用旧拖动。编辑中的文字操作点取消会恢复原草稿几何；序号放置取消恢复编号，删除放置草稿不会删除此前选中的对象。
- 新测试覆盖前方对象删除后的身份解析、目标删除/快照身份或类型不符时拒绝更新、选中状态保留、Core 撤销合并与铅笔保护。UI 控制器回归覆盖移动/缩放中撤销、删除和切换工具，取消无额外撤销记录。既有所有工具、马赛克取消、光标与编辑测试保持通过。
- 当前 workspace 410 passed / 30 ignored（target/annotation-gesture-identity-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-gesture-identity-clippy.log）；包含 Slint 编译，git diff --check 通过。本批没有新的原生截图或 Release 性能数据。
- Gesture 生命周期协调和原对象快照仍由 Registry 持有；绘制/折线/箭头预览的完整状态所有权、窗口与编辑组件合并、参数状态迁移、逐对象效果依赖及 GPU 端到端验收继续待完成。

## 编辑组件与实例状态合并

- Registry 不再独立持有 text_editor/text_edit 两份 Option。annotation_editor::State 的一个私有 active 实例同时拥有 AnnotationTextInput 组件及 TextEdit 元数据，活动窗口与草稿不能分别创建或清理。
- 打开/替换、提交取出、失败清理、会话关闭和所有者销毁共用实例 retire 路径：先使 active 不可访问，停止准备/光标计时，再隐藏并销毁组件。取出提交时返回确认文本与元数据；直接清理不额外复制整段输入文本。
- 参数更新、鼠标选区及排版更新通过 parts_mut 同时借用输入组件与草稿；没有为样式变化重新创建编辑组件。普通文字和序号说明共用该所有者，原有焦点、插入位置、选区、组字预览、布局和快捷键协调保持原调用次序。
- 新测试使用软件 WindowAdapter 与可控时钟，验证样式变化保留中文内容/字节选区；取出草稿、替换组件及直接销毁所有者后旧组件弱引用失效、旧准备/光标任务不运行；保留已取出的草稿数据也不能使任务继续。这是 Slint 组件生命周期证据，不替代真实原生窗口/输入法或 GPU 资源验收。
- 当前 workspace 411 passed / 30 ignored（target/annotation-editor-window-owner-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-editor-window-owner-clippy.log）；包含 Slint 编译，git diff --check 通过。现有字体、对象身份、参数后继续输入、取消和所有工具回归保持通过。
- 文字事件/显示准备/布局协调仍位于 Registry；其他窗口与水印编辑、参数所有权、绘制预览状态及 GPU 缓存/故障/端到端性能余项继续待完成。本批没有新的真实应用截图或 Release 性能数据。

## 水印窗口所有权与工具条实例隔离（2026-10-03）

- 新增 UI 内部 window::Slot，统一拥有临时组件、实例编号与显示准备计时器。水印输入窗口由该所有者管理；关闭、替换和销毁停止准备任务并隐藏/释放组件，菜单先关闭再释放其所属输入窗口。
- 窗口身份由 UI 线程单调分配，跨所有者重建不复用；二级工具条关闭也使用同一身份分配，避免 Registry 重建后旧工具条实例恰好匹配新实例。没有修改 Core 接口、依赖或配置。
- 水印原生鼠标转发在弱引用升级前检查会话及实例。二级工具条的原生事件、延迟参数更新、水印入口、Escape、关闭请求及分组选择补齐会话/实例校验。旧组件仍被其他代码强引用时也不能操作当前窗口；参数工具条保持展开和工具菜单外部关闭规则不变。
- 软件窗口/可控时钟测试验证关闭、替换和销毁取消准备任务以及新所有者身份不复用。UI 回归刻意保留旧水印/工具条组件并发送旧鼠标或回调，检查新菜单不关闭、旧参数不回写、不新开水印窗口、不新增撤销；覆盖100%/125%比例。该证据不替代真实 Windows 输入法、资源或多屏截图验收。
- 当前 workspace 412 passed / 30 ignored（target/annotation-window-instance-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-window-instance-clippy.log），包含Slint编译；git diff --check通过。
- 主工具条/画布/二级工具条尚未整体迁入窗口所有者；二级工具条显示重试仍是实例校验的独立延迟任务，后续需随所有者关闭立即取消。菜单结果接口、参数/绘制预览所有权、GPU增量失效和实际应用性能验收继续待完成。本批没有新的真实应用截图或Windows Release性能证据。

## 二级工具条窗口所有权（2026-10-03）

- Registry 的 panel Option 与独立 panel_revision 合并为 window::Slot<AnnotationPanel>。字体/参数回填、菜单所属关系、焦点请求和采集暂停区读取当前组件；实例身份仅由窗口所有者分配与失效，不能与实际组件分开修改。
- 首次显示与16ms原生就绪重试使用所有者的准备计时器。收起、替换、显示失败及直接销毁所有者立即停止任务；原会话/实例校验继续保留。二级参数工具条保持展开，分组菜单继续注册外部关闭监控。
- Slot 支持所属窗口的退休回调，二级工具条将关闭监控清理交给该回调；正常关闭及直接销毁均先失效身份、停止准备、移出组件，再清理监控并隐藏窗口。业务层仍在退休前完成参数事务和保存默认值，三级菜单先关闭，避免改变焦点及撤销顺序。
- 扩展可控时钟测试：保留旧面板强引用后 clear 仍清理一次监控；直接销毁所有者也清理一次，取消待执行准备。UI 既有回归验证旧参数/鼠标/Escape回调隔离、100%/125%比例、工具条展开、三级弹层、输入焦点及撤销。
- workspace 412 passed / 30 ignored（target/annotation-panel-owner-workspace.log），包含Slint编译。Clippy发现迁移产生的可合并嵌套if，已按等价条件表达式修复；workspace/all-targets Clippy -D warnings复查通过（target/annotation-panel-owner-clippy.log），git diff --check通过。未进行真实原生多屏/IME截图或Release性能验收。
- 主工具条/画布窗口、菜单请求/结果接口、参数与绘制预览状态所有权仍待迁移；GPU逐对象依赖、相同像素提交抑制、故障保留画面及端到端性能继续待完成。

## 主工具条窗口所有权与启动任务（2026-10-03）

- 主工具条从 Registry 的独立 Option 迁入 window::Slot。启动时先分配实例并建立会话身份，再安装组件回调；原生鼠标转发和工具/撤销/删除/路径/完成/Escape/窗口拖动/关闭的延迟动作共同校验会话与实例，旧组件强引用不会授权新会话操作。
- 画布启动的分步bootstrap、主窗口初次显示和16ms准备重试由主窗口 preparation 计时器拥有。跨屏DPI稳定后的32ms定位使用同一所有者的独立 placement 计时器，保持原调度间隔。清理、替换或直接销毁窗口立即停止两类任务；回调也继续校验实例。
- 新UI回归使用可控时钟并保留旧工具条：关闭重开后发送旧工具选择、完成、Escape、操作模式、撤销和原生点击，当前窗口/模式保持不变；旧定位不查询工作区，新工具条仍可打开参数面板。通用窗口测试同时验证关闭/销毁取消定位任务。软件WindowAdapter不提供真实物理定位，本测试以工作区查询和状态为证据，不声称验证真实跨屏布局。
- workspace 413 passed / 30 ignored（target/annotation-main-owner-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-main-owner-clippy.log），包含Slint编译；git diff --check通过。
- 画布组件与逐屏启动状态、主窗口活动期间的布局/放大镜轮询仍由Registry协调；WindowSet与纯菜单结果接口、参数/绘制预览所有权及GPU增量/性能余项继续待完成。本批没有新增真实GUI截图、Windows Release或性能证据。

## 画布组件、逐屏准备状态与输入身份（2026-10-03）

- 新增 annotation_canvas::Layer，每个屏幕组件共同拥有命中/光标反馈、展示帧、渲染签名、透明输入状态、启动阶段、实例身份与定位计时器。生产模块使用明确的 frame/window 依赖，不访问 Registry。
- 移除 Registry 中平行的 canvas_bootstrap 数组。分步启动直接读取对应 Layer.bootstrap，保留先创建所有屏幕、轮流推进未就绪屏幕的规则；调度通过状态迭代器执行，不额外复制逐屏状态列表。
- 画布32ms定位重试由 Layer 自己持有，销毁时停止并隐藏组件。原生鼠标、取消、Slint指针、滚轮及键盘/关闭回调校验会话、屏幕索引与独立实例；同一索引重建不能通过旧组件强引用重新授权输入。
- 新生命周期测试使用可控时钟验证保留旧组件时销毁仍取消定位、身份不复用及组件最终释放。UI回归在同一会话/同一索引重建后拒绝旧绘制、鼠标取消、Undo和Escape，新画布仍完成绘制并只产生一次撤销；既有100%/125%取消、负坐标、GPU透明输入和逐屏启动测试通过。软件WindowAdapter证据不替代真实多屏布局/IME/原生资源验证。
- workspace 415 passed / 30 ignored（target/annotation-canvas-owner-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-canvas-owner-clippy.log），包含Slint编译；git diff --check通过。
- Registry仍协调窗口组的启动顺序、屏幕查询、轮询和绘制；菜单请求/结果接口、参数与绘制手势所有权及GPU依赖/像素失效/故障/性能验收继续待完成。本批没有新的真实应用截图或Windows Release性能证据。

## 按窗口身份排除标注覆盖层（2026-10-03）

- WGC枚举移除“排除整个本进程”规则。请求中的具体窗口及平台标记的原生标注视觉被排除；本软件其他普通窗口可以采集。原生标记是HWND局部属性，不调用全局截图限制或改变其他录屏程序行为。
- GPU显示表面与原生放大镜host在显示前标记，销毁窗口后属性随HWND释放；标记失败销毁新窗口并沿用现有失败路径。GPU请求同时排除RenderSurface输入窗口，避免漏传场景排除列表时采回输入画布。
- UI增加GPU/软件共用的精确排除列表，覆盖画布、主/二级工具条、文字编辑、水印及三级菜单。软件请求不再依赖空排除列表和进程过滤；排除身份变化独立触发新的请求，即使ROI和暂停区域不变。保留工具条覆盖冻结与恢复规则。
- 新原生测试实际打开同进程普通源窗口和标记窗口：枚举保留前者、排除后者；普通源生成WGC GPU纹理；显式排除后销毁其采集器；stop_capture后数量归零。测试先建立Backend物理坐标上下文，修复当前125%DPI下测试窗口被虚拟化到错误ROI的问题。日志target/annotation-capture-scope-native-tests.log。
- 九项显式GPU原生回归通过（target/annotation-capture-scope-gpu-tests.log），覆盖画质、顺序、放大镜、暂停、原生显示与关闭。UI模拟Port测试验证四类已打开UI窗口被排除、相同策略不重复请求以及同位置更换画布时只因排除身份变化而重新请求。
- workspace 416 passed / 31 ignored（target/annotation-capture-scope-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-capture-scope-clippy.log），包含Slint编译；git diff --check通过。上述忽略项目中，本轮显式执行了新原生测试和九项GPU测试。
- 原生测试是拥有的受控窗口证据，不替代真实应用多屏/输入法交互或Release端到端性能。GPU逐对象依赖、相同像素提交抑制、故障保留画面及UI剩余职责迁移继续待完成；正常GPU路径未新增CPU屏幕读回。

## GPU局部效果采样依赖缓存（2026-10-03）

- 每个马赛克/放大镜输出块保存独立TileInput：会话和对象身份、采样边界、覆盖RasterKey、实际滤波/取景参数、屏幕源版本以及相交的下层内容依赖。不复制对象轨迹或CPU屏幕像素；镜头阴影/连接线等展示参数不进入取景参数，马赛克对象总边界和笔刷直径不额外使未变化的覆盖块失效。
- 完成滤波后分配输出版本；后续效果依赖实际相交的输出版本，形成按文档顺序传播的局部依赖。下层普通标注在采样边界外、上层标注及交互层变化可复用该块；删除的块同时清理输入和版本缓存。暂停块保留旧完整结果及旧输入，恢复后重新核对依赖。
- content::SampleRule供依赖追踪和GPU合成共同使用。马赛克采样之前的内容；放大镜采样全部普通标注和之前的马赛克，不采样其他放大镜，Erase仅使用屏幕底图。避免缓存判断与实际绘制维护两套采样范围规则。
- 扩展原生GPU测试，验证上层变化和范围外下层变化不推进效果版本；相交下层变化刷新像素并传递到后续相交效果，远处效果版本保持；镜头阴影参数变化在覆盖资产不变时保留取景版本。九项显式GPU测试通过（target/annotation-effect-dependencies-gpu-tests.log），含现有模糊校准、顺序、暂停、镜头与桌面显示回归。
- workspace 416 passed / 31 ignored（target/annotation-effect-dependencies-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-effect-dependencies-clippy.log），包含Slint编译；git diff --check通过。
- 屏幕源仍采用全局保守版本：任一WGC源帧变化时重新核对并更新所有未暂停块。本批改善文档变更的失效范围，不证明屏幕像素未变时不计算，也不证明GPU相同画面提交抑制或Release性能达标。按源ROI检测像素变化、设备失败画面保留、UI余项和真实应用验收继续待完成。

## 2026-10-02 基础改动验证

- workspace：375 passed、21 ignored，日志 target/annotation-refactor-foundation-workspace.log。
- workspace/all-targets Clippy -D warnings：通过，日志 target/annotation-refactor-foundation-clippy.log；UI构建同步完成Slint编译。
- 新增测试覆盖身份不复用、复制对象、删除后索引变化、参数替换身份保护、事务取消/删除归并、四类自定义颜色互不影响及渲染缓冲区尺寸/溢出校验。
- 此阶段没有GPU运行或新的Windows Release证据；不沿用旧Release声明新代码已实测。
- 下一步建立UI场景构建和局部图层缓存，并在Platform接入WGC GPU纹理与DirectComposition显示。所有完整目标保持待验收。

## GPU采集像素变化检测（2026-10-03）

- WGC新帧不再直接作为像素变化。每个源窗口只持有一个待比较GPU快照；计算着色器逐通道比较请求ROI的实际像素，未变化保留已发布纹理，变化后交换纹理及对应SRV。正常GPU路径仅非阻塞读取4字节完成标志，不读取屏幕纹理到CPU。
- 比较使用8×8线程组及组内汇总，按局部ROI派发，不扫描窗口中未请求的部分。结果未就绪时保留已确认纹理，不排队新候选；WGC仍只取有限数量的最新帧。软件采集保留原有即时复制行为。
- ROI改变时保留最新完整GPU快照；candidate_fresh区分新候选与交换后旧基线，避免扩区时把旧纹理重新发布。公共D3D编译工具供图形和计算着色器共用，无新依赖。
- 新原生D3D11测试通过：13×7非线程组整倍数纹理，四通道末端单像素变化、相等基线复用、区域外变化忽略、扩区重新检测及候选身份更新。日志target/annotation-gpu-change-native-tests.log。同进程WGC源/窗口排除回归通过（target/annotation-gpu-change-capture-tests.log），九项GPU效果与显示回归通过（target/annotation-gpu-change-gpu-tests.log）。这些是受控原生证据，不等于真实应用交互或性能验收。
- workspace 416 passed / 32 ignored（target/annotation-gpu-change-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-gpu-change-clippy.log），包含Slint编译；git diff --check通过。
- Windows workspace Release构建通过（target/annotation-gpu-change-release.log）；该结果仅证明当前代码可构建，不证明实时帧率或输入延迟达标。
- 异步比较增加一次确认阶段，其端到端延迟需要Release实测；相交ROI变化仍推进全局源版本，未实现逐效果源版本传播。窗口元数据或被遮挡源变化也可能使最终相同画面重新提交，最终合成像素提交抑制仍待验收。未新增真实应用DPI截图或完成三组30秒性能测试。

## 局部屏幕变化与效果源版本（2026-10-03）

- GPU比较结果由单一布尔标志改为16字节变化外接边界；逐组汇总真实变化像素，多次ROI派发合并边界。CPU只读取该元数据，不读回屏幕像素。初始化使用UpdateSubresource：原生测试发现RAW UAV清除会重复首个标量，不能用于不同的min/max初始值，已修复。
- 采集器将本地变化边界转换到物理桌面坐标，随纹理传入效果链路。窗口增删、顺序、位置或透明度变化保守失效旧/新窗口边界；ROI改变和首次纹理仍保守失效该源窗口。单窗口多个分离变化合并外接框，框内空隙可能额外失效，不声称逐像素最小范围。
- 每个马赛克/放大镜块拥有独立屏幕源版本；只有实际采样区域与变化边界相交才推进版本。版本在暂停检查前记录，暂停块保留输出，恢复后按累积版本刷新；删除对象时清理其源版本。屏幕变化与下层内容输出版本继续共同决定缓存有效性。
- 原生像素比较测试通过（target/annotation-source-regions-comparison.log），覆盖相同帧、四通道末端变化、ROI扩大及两次分离派发的精确外接边界。九项GPU回归通过（target/annotation-source-regions-gpu.log），新增实际源纹理局部变化、相交后续传播、远处结果复用和暂停恢复断言。同进程WGC源/窗口排除回归通过（target/annotation-source-regions-capture.log）。
- workspace 416 passed / 32 ignored（target/annotation-source-regions-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-source-regions-clippy.log），包含Slint编译；git diff --check通过。
- Windows workspace Release构建通过（target/annotation-source-regions-release.log）；未用构建通过代替运行性能验收。
- 尚未证明最终合成画面相同时不提交：被遮挡源变化和保守窗口边界可能导致多余展示。异步确认延迟、真实Release三组30秒性能、UI剩余职责迁移和故障保留画面继续待验收。

## 可见绘制指令与提交失效（2026-10-03）

- 将Effects的实际绘制顺序抽为借用型Draw指令迭代器；马赛克输出、镜头输出与装饰、普通栅格按同一指令流绘制，并生成PresentationInputs。记录可见RasterKey/边界、效果输出版本、顺序、会话及显示表面身份；不复制对象轨迹或CPU屏幕像素。
- 当前场景版本变化但可见指令相同，或源变化没有更新任何可见效果时，Renderer确认复用已显示内容，不再次清空/绘制交换链或调用Present。屏幕外指令不进入签名。只在原生提交成功后记住新签名；提交拒绝不推进签名，显示表面重建立即使旧签名失效。
- 扩展受控原生桌面测试：新分配但等价的文档版本及新屏幕外资产直接确认完成，提供panic提交回调确保没有调用原生提交；随后实际可见变化仍经过提交边界，取消后桌面保持之前画面，已显示版本不被拒绝帧修改。九项GPU原生回归通过（target/annotation-presentation-inputs-gpu.log），覆盖效果顺序、镜头、暂停、滤波画质与清除。
- workspace 416 passed / 32 ignored（target/annotation-presentation-inputs-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-presentation-inputs-clippy.log），包含Slint编译；git diff --check通过。
- Windows workspace Release构建通过（target/annotation-presentation-inputs-release.log），未进行本批端到端性能验收。
- 本批是基于真实绘制资产的提交失效，不是最终像素相等检测。效果输出版本推进但滤波结果相同、或遮挡导致相同像素时仍可能提交；这类情况以及Release端到端性能、真实GUI/DPI/IME、UI剩余所有权和故障保留画面继续待完成。

## 马赛克状态的明确输入与任务身份（2026-10-03）

- 移除annotation_mosaic_session生产代码的父模块通配导入。State::sync接收明确的采集Port和CapturePolicy（区域、暂停区、排除窗口）；State::poll_capture返回类型化变化/失败结果，文档回滚与画布刷新仍由协调入口执行。保持请求版本、部分帧采用、首次超时与失败保留最近成功帧规则。
- 软件场景提交迁入State::submit，接收Session、草稿、手势、起始对象与监视器迭代器，不从Registry读取这些字段。监视器列表在已有采集帧后才求值，保持原有惰性行为；快照仍不含撤销历史。本批未消除软件路径全部轨迹复制。
- 每个State分配UI内部独立Instance，提示计时回调共同校验会话/实例/提示版本；轮询另含可取消epoch，停止软件或清空采样后，旧令牌不能重新开启管线。Tasks所有者销毁时停止轮询和提示计时器，Worker仍由State字段持有并随退休释放。
- 扩展现有超时/最后成功帧回归：同一会话替换State并创建相同提示版本后，旧轮询不修改新polling状态、旧提示回调不清除新提示。新增停止软件回归验证轮询令牌退休而提示所属实例不变。已有UI回归继续覆盖采集策略/窗口排除、绘制撤销、草稿预览、取消和参数操作。
- workspace 417 passed / 32 ignored（target/annotation-mosaic-owner-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-mosaic-owner-clippy.log），包含Slint编译；git diff --check通过。
- Windows workspace Release构建通过（target/annotation-mosaic-owner-release.log）；不代表本批真实应用交互或GPU性能已验收。
- sync/poll/fail/cancel_pending等协调函数仍显式依赖Registry，未声称整个模块完全解耦。菜单结果接口、参数/绘制预览、窗口组活动任务、最终像素提交抑制、故障保留画面及真实性能/GUI验收继续待完成；本批没有新增真实原生多屏或IME截图。

## 原生Release吞吐基线与可中断精确等待（2026-10-03）

- 新增三组显式忽略的性能夹具，使用生产WindowsAnnotationRenderPort worker及同进程受控、非激活60Hz GDI变化源；1280×720强度100模糊、80逻辑像素长笔刷和四个重叠模糊各预热5秒、测量30秒。测试源窗口和输入窗口随夹具销毁，未修改全局截图限制或系统计时器分辨率。场景/遮罩在测试中构造，不是完整App/SceneCompiler/IME交互验收。
- 记录实际原生提交时间而非RenderCompletion复用确认；统计钩子仅cfg(test)且仅性能夹具打开时记录。确认适配器为NVIDIA GeForce RTX 4060 Laptop GPU；两轮每组源频率约60FPS，采集窗口数量5。计数是原生提交，不能当作扫描输出或唯一像素帧，也不包含输入到可见画面的时延。
- 初始三组提交频率约32FPS，定位到活动帧调度的Condvar超时等待。改为worker拥有的高分辨率waitable timer与auto-reset事件；submit/clear/drop同时唤醒事件，等待不持有Mailbox锁，计时器及事件通过RAII释放。计时器不可用/等待失败时记录一次并退回原条件等待，不调用timeBeginPeriod或做全局计时调整。

| 场景 | 修改前提交FPS | 修改后提交FPS | 修改后提交间隔P95 ms |
|---|---:|---:|---:|
| 大区域模糊 | 32.032 | 53.966 | 34.219 |
| 长笔刷 | 32.134 | 53.334 | 34.021 |
| 四层重叠 | 32.034 | 52.168 | 34.175 |

- 基线日志target/annotation-performance-before-timer.log；修改后日志target/annotation-performance-release-tests.log与target/annotation-performance-{large-blur,long-brush,four-overlaps}.txt。三个夹具完成不等于性能验收通过：提交FPS仍低于57，P95仍高于20ms。GPU执行P95和输入到可见P95明确为未测量，下一步需要独立阶段计时以区分采集/确认、GPU处理和显示等待。
- 十项GPU/调度原生回归通过（target/annotation-performance-gpu-regression.log），含等待可中断/复用、显示取消、关闭清除、效果顺序、镜头及滤波画质。workspace 417 passed / 36 ignored（target/annotation-performance-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-performance-clippy.log），包含Slint编译；git diff --check通过。
- Windows workspace Release构建通过（target/annotation-performance-workspace-release.log）。
- 未新增真实App多屏/DPI/IME截图或完成性能门槛；最终像素相等提交抑制、故障保留画面、UI剩余接口与完整验收仍待完成。

## 比较等待分类、短重试与固定帧节拍（2026-10-03）

- 性能夹具新增cfg(test)阶段统计：帧尝试间隔、CPU采集/效果调用/合成/提交时间、源未变化、采集和展示等待。GPU比较尚未完成的标记来自采集器的非阻塞Map结果；统计钩子只在显式性能记录时保存，不产生热路径日志，CPU阶段时间不当作GPU执行时间。
- 大模糊基线：约58.9次尝试/秒、CPU工作P95约1.45ms、无明显展示等待；169次源复用中136次伴随比较未完成（target/annotation-stage-profile-comparison.log）。说明完整帧周期等待掩盖了可尽快确认的新源，不以文件行数或滤波强度猜测瓶颈。
- 已显示当前版本而比较仍未完成时，保留已确认画面并用RenderWait::Source报告等待；worker短间隔重读元数据，不再次等满一帧。常规调度保持固定60Hz节拍，唤醒误差不逐帧累加；过期节拍丢弃，不排队追赶。短重试另付采集查询成本，最终等待预算500微秒；绘制和提交仍依靠已确认变化及版本授权。
- 单场景中间数据：短重试使大模糊提交P95从约34ms降至19.185ms，FPS56.668（target/annotation-stage-profile-retry.log）；固定节拍首轮FPS57.333/P95 20.176ms（target/annotation-stage-profile-phase.log）。保留中间未达标证据，不以一次接近门槛的结果作为完成。

| 最终30秒原生夹具 | 提交FPS | 提交间隔P95 ms | CPU工作P95 ms |
|---|---:|---:|---:|
| 大模糊 | 57.200 | 23.185 | 1.386 |
| 长笔刷 | 57.900 | 18.904 | 1.394 |
| 四层重叠 | 57.566 | 19.194 | 1.477 |

- 日志target/annotation-stage-profile-final.log；均为RTX4060 Laptop、Release、60FPS变化源。大模糊P95仍超过20ms，整体性能门槛未通过；两项其余夹具只通过原生提交频率/间隔子指标，不证明真实显示FPS、GPU P95≤12ms或输入到可见P95≤50ms。
- 十项GPU/调度原生回归通过（target/annotation-stage-profile-gpu.log），含取消提交、关闭/重建与效果顺序；workspace 417 passed / 36 ignored（target/annotation-stage-profile-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-stage-profile-clippy.log），包含Slint编译；git diff --check通过。
- Windows workspace Release构建通过（target/annotation-stage-profile-workspace-release.log）。
- 后续需GPU时间戳/队列阶段计时、确认大模糊尾部等待，以及验证展示表面未就绪时采集变化不会被提前消费。最终像素提交抑制、故障保留画面、UI剩余职责和真实App多屏/IME/性能验收继续待完成。

## 显示等待期间保留已确认源变化（2026-10-03）

- 根据当前Renderer顺序确认：sample_gpu先确认源变化，表面ready可能随后拒绝处理；效果已处理但commit返回false时，presented_stamp仍是旧值。源停止变化且场景stamp相同时，原有早退可能跳过尚未显示的新效果。
- 新增PendingFrame内部状态：待效果处理的变化范围与待显示标记分开确认。区域累计去重、上限64，超过上限保守合并；不保存像素帧或无限事件队列。效果处理成功后清除范围，原生提交成功或确认显示内容等价后才清除显示标记；处理失败仍保留范围。
- 无实时效果时清除源处理范围，仍完成需要的显示更新。移除GpuCapture冗余changed布尔值，以changed_regions作为唯一变化依据；没有新增Core接口、配置或依赖。
- 两项纯回归覆盖等待后区域保留、提交失败后仍有显示债务、区域独立失效和长时间等待下有界元数据。新增显式忽略的原生D3D11回归：同stamp源由红转绿一次，注入表面未就绪，随后两次无变化样本分别拒绝/允许提交；验证仍进入提交、重试不重算效果、读回实际效果纹理全为绿色，稳定后不重复提交。
- 该夹具使用受控GPU源纹理和所属原生输入窗口，故障注入仅cfg(test)。验证了生产Renderer的处理/提交顺序，不证明真实WGC源、多屏、实际显示或应用输入验收；未新增真实应用截图。
- workspace 419 passed / 37 ignored（target/annotation-pending-frame-workspace.log），含Slint编译；11项原生GPU/调度回归通过（target/annotation-pending-frame-native.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-pending-frame-clippy.log）；git diff --check通过。
- 本批未重新测量性能，大模糊P95 23.185ms缺口仍未关闭。继续检查多窗口采集中途等待时的变化保留，GPU时间戳、最终像素相等提交抑制、故障保留画面、剩余UI职责与真实App验收仍待完成。
- Windows workspace Release构建通过（target/annotation-pending-frame-release.log）。

## 多窗口采集等待与共用变化范围（2026-10-03）

- 当前Backend::sample_gpu逐窗口确认纹理变化，但若后续窗口缺少首帧，原本立即返回None并丢弃局部changed_regions。窗口列表签名也已经更新，下一轮不会重发同一拓扑变化。
- Backend现在拥有待交付gpu_changes：拓扑/像素变化立即合入，缺帧或临时错误不清除；轮询所有可见源，只有完整非空样本交付时才转移给Renderer。停止采集同时释放源与清空待交付范围。不排队缓存像素帧。
- 将上批PendingFrame的64区域上限/去重/保守合并抽为screen_capture::regions::ChangedRegions，采集和效果处理共享实现。范围在正常未阻塞时仍逐区域传递，避免重复实现或默认整屏失效；无新增Core公开接口、配置或依赖。
- 扩展已有原生WGC窗口回归：两个所属普通窗口、标注窗口排除规则保持正常；测试注入一个窗口暂不可交付，确认另一源仍获得纹理，单次GDI绘制后保持源静止，解除等待时完整样本包含全部累计区域；交付及stop_capture后待范围为空。实际WGC生产者参与；延迟交付注入只cfg(test)，不声称覆盖全部实机时序或真实应用。
- 原生WGC定向测试通过（本轮工具执行输出）；11项GPU/调度回归通过（target/annotation-capture-pending-native.log）。workspace 419 passed / 37 ignored（target/annotation-capture-pending-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-capture-pending-clippy.log）；git diff --check通过。
- 没有新增真实性能或App截图证据；大模糊P95 23.185ms、GPU/输入显示计时、最终合成像素相等、设备失败画面保留、剩余UI职责及真实多屏/IME验收继续待完成。
- Windows workspace Release构建通过（target/annotation-capture-pending-release.log）。

## GPU时间戳计时与处理边界（2026-10-03，测量继续）

- 新增仅cfg(test)的gpu_timing，性能夹具显式开启后才创建查询。最多8组disjoint/timestamp查询，非阻塞读取并回收复用；满时跳过测量并记录丢弃数，不拖慢输入等待GPU。时钟不连续、频率为零、时间倒退均不生成有效样本；未完成查询单独计数。
- GetData使用原始HRESULT区分S_FALSE，避免windows生成的Result包装将“未完成”当作成功。使用DONOTFLUSH读取；性能路径不因查询增加Flush，孤立原生查询回归自行Flush以验证回收。
- 文档依据：[Microsoft D3D11查询](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_query)、[时间戳有效性](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ns-d3d11-d3d11_query_data_timestamp_disjoint)。GPU时间是设备命令时间线（含供给间隙），不是shader活跃时间。
- 首轮诊断日志target/annotation-gpu-timestamp-performance.log：查询结束标记放在Present后，尾段约15–17ms，而效果段约3.7–5ms；该结束标记可等待下一次命令提交，不能据此把总值18ms解释成GPU滤波耗时。首轮还有并行构建负载，保留诊断数据，不作为最终吞吐验收。
- 已将完整处理段结束标记移到原生提交前；早退尝试独立记账，不混入完整帧GPU P95。结果被CPU观察到的延迟包含轮询间隔，明确不当作队列延迟或输入到显示延迟。现有CPU提交计时继续保留。
- 查询时钟纯回归与有界/非阻塞/两轮复用原生回归通过；12项原生GPU/调度回归通过（target/annotation-gpu-timestamp-final-native.log）；workspace 420 passed / 38 ignored（target/annotation-gpu-timestamp-final-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-gpu-timestamp-final-clippy.log）。

### 修正处理边界后的三组30秒结果

| 原生夹具 | 提交FPS | 提交间隔P95 ms | 提交前GPU命令处理P95 ms | GPU效果段P95 ms |
|---|---:|---:|---:|---:|
| four-overlaps | 56.934 | 31.195 | 5.206 | 4.833 |
| large-blur | 57.266 | 19.749 | 4.485 | 3.671 |
| long-brush | 57.002 | 30.061 | 5.640 | 5.171 |

- 最终日志target/annotation-gpu-timestamp-final-performance.log：RTX4060 Laptop，源均约60FPS，构建及其他原生验证已结束后开始测量。查询丢弃/无效均0，测量结束仍有1–2组未完成查询，完整GPU样本数逐场景报告；未完成不记为0ms。
- GPU处理子指标低于12ms；本轮大模糊通过原生提交FPS/间隔及GPU处理三个子指标，但长笔刷与四层重叠提交间隔P95超过20ms，四层重叠FPS也低于57。整体性能未达标；不能用该GPU子指标证明实际显示FPS或输入到显示≤50ms。与前轮数据有波动和查询开销，保留全部结果，不挑选通过的轮次。
- 后续需定位每个WGC源的帧到达、比较确认和队列等待；现在的结果观察延迟包含轮询，不能据此宣称已测GPU队列等待。UI接口收窄、最终像素等价提交抑制、设备降级画面保留及真实App多屏/IME/性能验收仍未完成。
- Windows workspace Release构建通过（target/annotation-gpu-timestamp-final-release.log）；git diff --check通过。未暂存、提交或推送。

## 菜单显示能力边界与不可复用实例（2026-10-03）

- AnnotationChoice的Menu::show/ place现在只接收已解析的所属窗口及最小原生能力：准备、配置、所属关系、显示完成、关闭监控、工作区查询。不读取Registry、文档、参数或编辑状态。协调入口负责身份检查、请求转换、显示失败后的事务/焦点处理与有界重试，保留原有准备/显示/桥接顺序和DPI重新定位。
- State::take改用现有window::next_revision分配唯一编号，避免同一会话/同一panel内替换菜单State后复用局部编号。State::matches集中校验本实例及return_pending；会话、panel和水印实例由协调入口继续校验。
- 新增独立菜单显示回归：无需初始化REGISTRY文档状态，Pending不调用原生显示能力，Ready按原有顺序配置/附属/桥接/监控；验证负坐标工作区翻转和限制。另新增同会话/panel替换State回归，保留旧组件强引用后旧show/finish/outside回调不能选择或关闭新菜单。
- 10项菜单回归通过（target/annotation-menu-presentation-tests.log），覆盖自定义色板/参数目标、字体定位/键盘导航、两种DPI/负坐标布局和窗口销毁；workspace 422 passed / 38 ignored（target/annotation-menu-presentation-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-menu-presentation-clippy.log）；git diff --check通过。
- 本批只收窄显示/定位边界，菜单请求/结果及参数应用协调仍访问Registry，未声称菜单模块完整解耦。GPU供帧与队列定位、最终像素提交抑制、降级画面保留及真实App交互/性能验收继续待完成。本批未新增实机截图或重测GPU吞吐，没有暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-menu-presentation-release.log）。

## 菜单返回明确参数变更（2026-10-03）

- 在既有annotation_parameters纯映射模块中新增ChoiceTarget、TextStyleTarget和Edit。普通选择先校验取消/索引/禁用项/未知字段，再产生KeepOpen/Close/Apply结果；原有荧光笔禁用项规则保持不变。
- 描边/背景在Slint回调入口解析目标，只把自身宽度/颜色或不透明度/圆角/内边距/颜色打包为变更，不再把整份AnnotationValues快照放入延迟任务。Edit对最新工具条值应用，保留期间输入的说明、字号、字体及其他效果。
- Menu::edit_color只解析/更新自身HSV、HEX、预览和错误提示，返回指定ColorTarget的变更，不读取Registry或文档。普通选择、颜色和文字样式统一交给协调层Registry::apply_menu_edit；字体手动偏好及原有工具11对象更新通知也在协调层处理。菜单不再依赖字体目录模块或保留另一套字段写入表。
- 两项纯回归验证延迟变更保留期间输入和另一种效果更新，以及取消/越界/未知/禁用项选择；10项菜单回归通过（target/annotation-menu-edit-tests.log），原有自定义色/参数目标、字体定位、关闭返回和实例隔离继续通过。workspace 424 passed / 38 ignored（target/annotation-menu-edit-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-menu-edit-clippy.log）；git diff --check通过。
- 这是结果与参数写入边界迁移；菜单构建/导航/返回、事务及焦点协调入口仍访问Registry，默认参数和事务尚未迁入独立所有者。GPU供帧/队列等待、最终像素提交抑制、降级画面保留及真实App多屏/IME/性能验收仍待完成。本批没有新增实机截图或GPU吞吐测量，没有暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-menu-edit-release.log）。

## 连续参数事务与可取消结束计时器（2026-10-03）

- 新增UI内部annotation_parameter_transaction::Transaction，收拢活动标记、工具条拖动标记、操作版本及唯一300ms空闲计时器。Registry移除原有style_edit_active/parameter_dragging/style_edit_revision字段；重复更新重置同一计时器，完成/窗口关闭/所有者销毁取消任务，不再每次更新创建不可直接取消的single_shot。
- touch接收明确Target（panel实例、工具、ObjectId、编辑实例）及Core Session；相同目标继续使用已有Core拖动快照，目标改变时先结束上一操作。版本采用现有唯一编号，替换所有者后旧令牌不能匹配新状态。Core仍负责实际快照、无变化判断与撤销历史，UI状态不复制对象或历史。
- 协调入口继续校验会话，并把菜单拖动状态传入ready_to_finish；工具条或选色器按住时不通过空闲回调结束。鼠标松开、关闭面板及现有完成路径仍调用统一finish并更新撤销提示/恢复焦点。
- 新增Core行为回归：连续多次更新合并，目标变化分成两条撤销，重复结束和无变化操作不增加历史，旧版本及拖动状态不能结束当前操作。模拟时钟回归验证最新任务300ms执行一次、重置/完成/销毁取消任务以及相同目标替换后的令牌隔离。
- 长滑杆/马赛克对象撤销与面板回归通过（target/annotation-parameter-owner-targeted.log）；2项事务所有者回归通过（target/annotation-parameter-owner-tests.log）；workspace 426 passed / 38 ignored（target/annotation-parameter-owner-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-parameter-owner-clippy.log）；git diff --check通过。
- 默认参数目录、对象回填、绘制预览及窗口组活动任务仍需继续迁移；GPU供帧/队列等待、最终像素提交抑制、设备降级画面保留与真实App多屏/IME/性能验收未完成。本批不改变Core公开接口、配置或依赖，没有暂存、提交或推送，也没有新增实机截图或GPU吞吐测量。
- Windows workspace Release构建通过（target/annotation-parameter-owner-release.log）。

## 默认参数与借用对象回填（2026-10-03）

- defaults、文字尺寸、对象和水印投影统一迁入 annotation_parameters；只接收借用对象、字体索引、下一编号及聚光灯透明度，不访问 Registry 或窗口。窗口通知和工具选择继续由协调层负责。
- adopt_selected_style 移除整对象 clone，不再在回填时复制轨迹。字体索引仅查目录，不调用手动偏好选择；保留原有工具/DPI/样式数值规则。正文、自定义描边和背景颜色统一使用现有颜色映射。
- 两项新增回归覆盖文字效果、自定义 RGB、字体索引、尺寸舍入、马赛克创建 DPI 和水印模板/清除后的默认值保留。workspace 428 passed / 38 ignored（target/annotation-parameter-refill-workspace.log），含 Slint 编译；Clippy workspace/all-targets -D warnings通过（target/annotation-parameter-refill-clippy.log）；git diff --check通过。
- 本批完成纯映射边界；默认值所有权与对象投影来源仍需继续明确，菜单协调及可见场景轨迹复制仍待收拢。GPU调度、最终像素提交抑制、故障画面保留及真实App性能/DPI/IME验收继续未完成。本批未新增实机截图或重测吞吐，没有暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-parameter-refill-release.log）。

## 未编辑画布借用文档（2026-10-03）

- visible_session现在仅在普通文字/序号说明存在未提交编辑投影时构建快照；无编辑时返回None，场景编译、软件绘制及签名直接借用当前Core Session，不再因每次刷新复制全部对象轨迹和显示列表。
- 软件采集/放大镜协调先完成可能改变文档的同步，再重新生成编辑投影或借用最新文档，保持首次软件失败回滚后的绘制一致。后台软件任务仍拥有异步快照，未将借用跨线程传递。
- 31项标注交互回归通过（target/annotation-borrowed-scene-tests.log），包含序号放置/取消、原生失败与软件回退、窗口生命周期、文字编辑和长滑杆事务。workspace 428 passed / 38 ignored（target/annotation-borrowed-scene-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-borrowed-scene-clippy.log）。
- 编辑中的全对象投影与软件工作任务复制仍需迁移为明确的只读场景数据；本批只移除无编辑热路径的无条件快照，不声称完成全部复制优化。最终像素提交抑制、GPU调度、故障画面保留及真实App性能/DPI/IME验收继续未完成；未新增实机截图或性能数字，未暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-borrowed-scene-release.log）；git diff --check通过。

## 软件任务在快照前去重（2026-10-03）

- 软件效果提交原先先复制所有对象/轨迹，再在Worker内部判断相同key/context/capture.serial。现在先借用文档和草稿计算效果内容签名，Worker::needs_submission在跨线程快照创建前判断重复；提交入口仍保留同样的去重守卫。
- 抽出document_signature，复用原画布签名规则但显式排除选中/悬停；与原有无选中任务快照的签名相同。手势取消上下文只借用文档并替换一个归一化对象，保留样式/参数、手势epoch及其余文档变化，不再临时替换整个Session快照的对象。
- 新增回归验证借用签名等于旧快照签名、选择不使效果失效、参数变化使签名变化，以及相同任务拒绝/内容或上下文或采集serial改变后重新提交。既有几何归一化不修改实际任务、移动/缩放反馈、撤销取消及跨屏效果回归继续通过。
- workspace 430 passed / 38 ignored（target/annotation-software-dedup-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-software-dedup-clippy.log）。
- 此优化省去重复任务的快照，实际新任务仍拥有跨线程Session对象副本，处理时仍逐层复制下层对象；编辑投影、不可变工作场景及最终像素提交抑制尚未完成。没有重测吞吐或真实App输入显示延迟，GPU调度/降级画面及多屏/IME验收仍待完成；未暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-software-dedup-release.log）；git diff --check通过。

## 渲染签名去除Debug字符串（2026-10-03）

- 新增UI内部annotation_fingerprint，直接对Extra、文字/序号说明、样式及水印字段做结构化哈希，替换画布/软件任务/局部效果反馈签名中的format Debug临时字符串。只影响失效标识，不改变对象、字体排版、撤销或像素绘制。
- 使用完整字段解构和穷尽枚举匹配，新字段/变体必须补齐处理才能编译。浮点使用位表示，选项显式区分有无，枚举保留变体身份；签名不用于持久化或安全校验。
- 新增回归覆盖连接箭头/控制点、旋转、布局scale、描边、背景、字体及中文内容变化，并验证Arrow与Polyline不同变体不会被当作同内容。既有逐层效果、跨屏、原生故障回退及场景缓存回归继续通过。
- workspace 431 passed / 38 ignored（target/annotation-fingerprint-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-fingerprint-clippy.log）。
- 无新增性能数值或实机截图；编辑/异步工作场景轨迹副本、最终合成像素相等提交抑制、GPU调度、故障画面保留及真实App性能/DPI/IME验收仍待完成。未暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-fingerprint-release.log）；git diff --check通过。

## 共用GPU像素比较与候选接受规则（2026-10-03）

- 将screen_capture私有change迁为Windows内部d3d_compare，采集器复用原有精确compute shader和16字节变化范围读回；无需跨Adapter依赖、CPU像素读回、新crate或配置。为最终合成比较提供同一设备侧能力，当前尚未接入最终提交路径。
- 轮询返回明确Outcome：Idle/Pending/Equal/Changed；Pending不会被解释为像素相同。普通publish要求比较已完成；采集ROI策略变化通过显式publish_candidate接受更新的完整候选，并立即销毁旧比较任务，保持设备命令顺序及现有范围扩大恢复行为。
- 初次原生组测试发现新增publish断言与采集ROI切换提前接受候选的既有行为冲突，造成暂停恢复夹具失败；已分离上述两条接受路径并修复。首轮gpu_过滤误包含三个要求Release的性能测试，它们拒绝Debug运行；该日志保留为诊断，不记录性能结果。
- 修正后12项原生GPU/比较/调度回归通过（target/annotation-shared-comparison-gpu-final.log），覆盖精确边界、多区域范围合并、未就绪/提交拒绝、暂停恢复及桌面合成顺序。workspace 431 passed / 38 ignored（target/annotation-shared-comparison-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-shared-comparison-clippy.log）。
- 后续为每个显示表面保留已接受像素基准；比较候选时禁止旧/拒绝提交更新基准，并以明确异步等待重试接线。当前命令元数据去重仍不代表最终像素相等，整体需求未完成。没有新Release吞吐或真实App显示延迟/DPI/IME证据；未暂存、提交或推送。
- Windows workspace Release构建通过（target/annotation-shared-comparison-release.log）；git diff --check通过。

## 最终合成像素去重接入（2026-10-03）

- 新增平台内部annotation_gpu::output::Pixels，每个表面保留设备端已接受基准/候选纹理和16字节读回；最终合成按共用精确比较器异步检查。命令元数据不同但最终像素相同时，标记当前版本已呈现而不调用原生提交；未完成比较报告Source等待，不能被当作相同。
- 基准只在版本围栏和原生提交成功后推进；提交拒绝保留候选结果用于重试。新版本等待旧比较资源完成后重新合成，旧结果不会授权新场景；表面重建清理对应基准。未读回整帧像素。
- 扩展真实桌面夹具：更换前景资源版本，并在不透明前景下放置颜色不同的下层资产，最终像素相同则提交回调不得触发。单次源变化/表面未就绪/拒绝提交回归改为允许有界异步比较等待，仍要求静止源到达提交、失败可重试且效果不重算。
- 首轮回归失败来自旧测试一次调用即提交的同步假设，另一次来自新增场景与后续拒绝场景复用revision；已修正有界等待和唯一版本号。最终12项原生GPU/比较/调度回归通过（target/annotation-output-pixels-native-final.log）。workspace 431 passed / 38 ignored（target/annotation-output-pixels-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-output-pixels-clippy.log）；Windows workspace Release通过（target/annotation-output-pixels-release.log）。
- 当前像素比较引入等待，必须重测性能；旧提交吞吐不可代表当前代码。原GPU计时的完整帧标记尚未覆盖跨尝试的合成/比较/提交阶段，新的GPU处理P95不可直接与旧值比较；需继续修正计时边界。真实App输入显示延迟、故障降级画面保留、UI状态与轨迹副本及DPI/IME验收仍未完成，未暂存、提交或推送。

### 输出比较接入后的Release性能与短重试修正

- 首轮target/annotation-output-pixels-performance.log：four-overlaps 5.900FPS/366.618ms P95，large-blur 6.433FPS/283.733ms，long-brush 6.633FPS/283.780ms。原因是Source短重试仅对presented=true生效，输出比较等待返回false后每次等待完整刷新周期，候选反复被新采集版本作废。
- 已将Source短重试覆盖尚未呈现的比较等待；重新通过12项原生回归、workspace、Clippy和Windows Release。修正后三组完整30秒日志target/annotation-output-pixels-retry-performance.log：four-overlaps 35.468FPS/53.196ms P95，large-blur 34.733FPS/50.313ms，long-brush 33.734FPS/51.208ms。测试进程成功结束仅证明夹具完成，不代表性能门槛通过。
- 比较仍在变化源下作废候选，GPU/采集/输出读回之间的等待未形成有效流水线。当前吞吐低于原有约57FPS，必须继续优化；不能宣称本批提升实时性能。下一批需按场景版本隔离候选授权与新采集债务，避免源帧变化导致已完成的输出比较持续作废，同时维持最新帧和对象版本校验。跨尝试GPU计时也需修正，当前GPU数值不纳入达标结论。
- git diff --check通过；原始和修正日志均保留。没有实际App输入到显示或真实多屏/IME验收证据，整体目标保持未完成。

## 比较候选与新采集显示债务分离（2026-10-03）

- output::Pixels为候选保存不可变RenderStamp；同场景版本下允许完成冻结候选的精确比较/提交，新源纹理到达不再持续作废它。场景版本不同则丢弃旧比较结果并重新合成，版本围栏仍负责提交授权，不引入帧队列。
- 接受/相等确认记录候选的实际PresentationInputs，而非当前已准备的最新源输入；只有候选覆盖最新输入时才清除PendingFrame显示债务。否则保留债务并短重试，已准备的新效果随后合成。相等候选确认后释放候选元数据但不改变接受基准，避免持续复用旧相等结果吞掉新帧。
- 扩展原生单次源回归：输出比较开始后再次注入源变化，冻结候选仍到达提交，验证已呈现元数据属于该候选、新源债务保留，随后无进一步源变化仍提交最新准备结果并清除债务。12项原生GPU/比较/调度回归通过（target/annotation-output-source-debt-native.log）。workspace 431 passed / 38 ignored（target/annotation-output-source-debt-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-output-source-debt-clippy.log）；Windows workspace Release通过（target/annotation-output-source-debt-release.log）。
- 正在以当前Release重测三组30秒场景；旧34–35FPS仅为上批代码的结果。跨尝试GPU计时、实际App输入显示、多屏/IME、故障降级画面与剩余UI/场景职责未完成；未暂存、提交或推送。

- 当前Release三组完整30秒进程已成功结束（target/annotation-output-source-debt-performance.log）：four-overlaps 56.066FPS/33.668ms P95；large-blur 53.332FPS/36.223ms P95；long-brush 53.632FPS/35.721ms P95。候选授权分离恢复了吞吐，但整体仍未通过≥57FPS/P95≤20ms门槛。GPU跨尝试计时尚未修正，因此本轮不作GPU耗时达标结论；无真实App输入显示证据。
- git diff --check通过。下一步优先让待输出比较的完成/提交先于重复源枚举与效果更新，定位尾延迟并修正GPU计时，再重测。

## 输出候选重试先于重复采集（2026-10-03）

- Renderer对同场景待确认候选先执行resolve_output，不再在每次比较等待/提交重试前重复枚举WGC源和处理效果。新源仍由采集端最新帧机制保留，候选完成后下一次采样获取；不同场景继续走版本失效/重新准备路径。
- 比较等待、相等确认、提交 readiness、版本围栏回调、成功后的基准及债务更新统一进入resolve_output。提取的是实际候选提交职责，没有另建渲染状态机或跨线程帧队列；保留多表面ready许可和失败重试。
- 定向原生回归调整并增强：比较期新增源帧必须保持未消费；候选提交后再采样形成显示债务，确认后才清除。12项原生GPU/比较/调度回归通过（target/annotation-output-first-native.log），包含场景版本作废、遮挡等价及拒绝提交；workspace 431 passed / 38 ignored（target/annotation-output-first-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-output-first-clippy.log）；Windows workspace Release通过（target/annotation-output-first-release.log）。
- 正在重测当前Release三组30秒。GPU计时的跨尝试边界尚未修正，打印的GPU子指标不可作为完整处理耗时验收；整体目标还包括真实App输入显示、故障降级、UI/场景所有权及DPI/IME验收，均未以本批门禁替代。未暂存、提交或推送。

### 串行输出优先实验撤回，保留统一确认入口

- 输出先于全部采集的完整Release实验（target/annotation-output-first-performance.log）为four-overlaps 51.435FPS/35.405ms P95、large-blur 50.565FPS/34.753ms、long-brush 51.067FPS/34.650ms，没有优于上批53–56FPS。该调度顺序已撤回，未作为优化保留。
- 当前恢复采集与候选比较的并行推进，保留resolve_output对比较等待、相等确认、ready许可、授权提交、基准与显示债务更新的统一协调。不同场景版本作废及拒绝提交不更新基准的规则不变。
- 撤回后定向回归重新验证候选期间的新采集形成显示债务，随后静止源仍提交最新效果。12项原生GPU回归通过（target/annotation-output-resolution-native.log）；workspace 431 passed / 38 ignored（target/annotation-output-resolution-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-output-resolution-clippy.log）；Windows workspace Release通过（target/annotation-output-resolution-release.log）。
- 正在重测恢复后的当前代码，不把撤回前的实验数据当作当前性能。GPU计时仍需按实际合成/比较阶段校准；整体性能和真实App/DPI/IME/降级及UI职责验收未完成。未暂存、提交或推送。
- 恢复后当前Release三组完整30秒已结束（target/annotation-output-resolution-performance.log）：four-overlaps 55.536FPS/34.056ms P95，large-blur 54.601FPS/35.942ms，long-brush 54.133FPS/34.944ms。恢复到此前吞吐附近，仍未通过57FPS/20ms门槛；GPU跨尝试计时和真实显示延迟继续未验收。git diff --check通过。

## 完整处理尝试的GPU计时边界（2026-10-03）

- d3d_compare新增enqueue入口：只排入比较/元数据读回命令，不提交；采集begin仍显式Flush保持原有能力。输出比较使用enqueue，Renderer在完整处理结束标记之后统一执行生产Flush，多表面比较也在同一批提交，不为查询增加单独Flush或同步等待。
- GPU查询增加独立输出比较区间，六个时间点对应采集、效果、合成、输出比较及结束标记五个区间。完整样本须经过比较结束点；提交重试/相等确认/源等待等不完整尝试独立记录，不混入完整GPU处理P95。CPU合成和比较入队也分别报告。
- 范围明确为一次完整采集/效果/合成/比较尝试，包含命令供给间隙；它不是shader活跃时间、查询结果被CPU观察到的延迟、最终显示或输入到光子延迟。不再把跨尝试等待中的提交阶段当作已执行的完整处理。
- 12项原生GPU/比较回归通过（target/annotation-gpu-processing-boundary-native.log）；时间戳有界/非阻塞/复用回归通过（target/annotation-gpu-processing-boundary-query.log）。workspace 431 passed / 38 ignored（target/annotation-gpu-processing-boundary-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-gpu-processing-boundary-clippy.log）；Windows workspace Release通过（target/annotation-gpu-processing-boundary-release.log）。
- 正在对当前Release做三组30秒计时/吞吐重测；不能用旧日志证明新范围达标。UI/场景职责、故障降级画面及真实App输入显示/DPI/IME验收仍未完成。未暂存、提交或推送。

- 校准后当前Release三组完整30秒已结束（target/annotation-gpu-processing-boundary-performance.log）：
- four-overlaps: 55.099FPS，提交间隔P95 34.243ms，GPU完整处理P95 5.162ms，输出比较P95 0.137ms，查询丢弃/无效 6/0，结束未完成 5。
- large-blur: 54.101FPS，提交间隔P95 35.125ms，GPU完整处理P95 3.698ms，输出比较P95 0.233ms，查询丢弃/无效 0/0，结束未完成 3。
- long-brush: 54.401FPS，提交间隔P95 36.025ms，GPU完整处理P95 3.713ms，输出比较P95 0.233ms，查询丢弃/无效 0/0，结束未完成 2。
- GPU完整处理子指标低于12ms，但提交FPS/间隔仍未通过57FPS/20ms门槛；实际App显示及输入延迟仍未验收。输出比较没有整帧CPU读回，结束标记P95约0.001ms，不再等待下一次Flush形成虚高尾段。下一步定位采集到达/确认与调度等待，不以GPU子指标代替整体。git diff --check通过。

## 表面许可等待的短重试（2026-10-03）

- 当前校准日志在每组三十秒中有数百次Presentation等待，而调度只给Source等待短重试。显示许可可能在完整刷新期限之前到达，旧规则仍延迟下一次尝试，存在跨刷新尾延迟。
- 对Presentation增加1ms的可中断高精度定时重试；Source保持500us，Paused/无等待保持正常期限。next_attempt仍取正常期限与重试期限的较早者，不添加追赶帧、忙轮询或进程全局计时分辨率改变。
- 12项原生GPU/比较/调度回归通过（target/annotation-presentation-retry-native.log），覆盖许可保留、提交拒绝、暂停及单次源变化债务。workspace 431 passed / 38 ignored（target/annotation-presentation-retry-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-presentation-retry-clippy.log）；Windows workspace Release通过（target/annotation-presentation-retry-release.log）。
- 正在重测当前Release吞吐/尾间隔及尝试次数，不把代码推断当作性能收益。整体目标、真实App输入显示、故障降级画面、DPI/IME及UI/场景职责验收仍未完成；未暂存、提交或推送。

### 定时许可重试撤回，改用原生许可事件

- 1ms Presentation定时重试的三组完整Release结果（target/annotation-presentation-retry-performance.log）：four-overlaps 54.268FPS/44.207ms P95、large-blur 55.102FPS/42.869ms、long-brush 55.135FPS/44.605ms；尝试约273–306次/秒，没有改善尾延迟，该分支已撤回。
- 当前调度器在Presentation等待时将尚未持有许可的DXGI延迟事件加入会话信号/刷新定时器的同一次WaitForMultipleObjects。原生表面事件唤醒后按索引保留已消费的许可，立即允许重试；没有定时高频轮询或重复消费许可。最多等待62个表面事件，保留系统64句柄上限和原定时/信号退路；renderer及延迟句柄始终由同一worker拥有直到等待结束。
- Source仍用原有500us非阻塞元数据重试。会话结束/替换信号打断等待，下一轮正常生命周期清理原Renderer；设备错误清除Presentation等待状态，平台通用弹层机制及Core接口不变。
- 12项原生GPU回归通过（target/annotation-surface-event-native.log）；扩展原生等待回归验证事件索引、自动复位只消费一次、信号中断与定时器复用（target/annotation-surface-event-wait.log）。workspace 431 passed / 38 ignored（target/annotation-surface-event-workspace.log），含Slint编译；Clippy workspace/all-targets -D warnings通过（target/annotation-surface-event-clippy.log）；Windows workspace Release通过（target/annotation-surface-event-release.log）。
- 正在重测原生事件版本的三组30秒，以上定时版本不能作为当前收益。真实App显示/输入、故障降级画面、UI/场景职责和DPI/IME验收仍未完成；未暂存、提交或推送。

### 原生许可事件实验撤回及恢复确认（2026-10-03）

- 原生事件版本的三组完整30秒Release结果（target/annotation-surface-event-performance.log）：four-overlaps 53.801FPS/45.178ms P95，large-blur 50.833FPS/44.329ms，long-brush 55.868FPS/45.268ms。未改善此前约34–36ms尾间隔，事件等待分支已撤回；不保留无收益的调度复杂度。
- 已移除本轮新增的Presentation等待状态、表面事件收集/许可转移及wait_surfaces。恢复原有可中断定时/会话信号等待，Source仍保留500us短重试且覆盖未提交尝试。未撤回候选授权、新源显示债务、最终像素比较或校准计时。
- 恢复验证已完成且进程终止：12项原生GPU回归通过（target/annotation-permit-experiments-restored-native.log）；workspace 431 passed / 38 ignored（target/annotation-permit-experiments-restored-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-permit-experiments-restored-clippy.log）；Windows workspace Release通过（target/annotation-permit-experiments-restored-release.log）。计划核对时只读取这些终态日志，未重复运行。
- 恢复代码没有新增吞吐测试，不把失败实验数据当作恢复版本性能。下一批依据校准基线测量各采集源到达、比较确认与调度等待，定位长间隔原因后再改一个阶段。真实App显示/输入、故障降级画面、UI/场景职责和DPI/IME验收仍未完成；未暂存、提交或推送。

## 逐源确认观测及入队等待修正（2026-10-03）

- 新增screen_capture::diagnostics，仅cfg(test)启用：记录源序列、取帧、比较确认耗时及Pending轮询。每源缓存最多128项、测量邮箱最多131072项，丢弃显式计数；不逐帧打印，不增加生产计时、Core接口、配置或依赖。预热也清空源缓存，正式统计只保留测量窗口内的事件。
- 新回归验证有界缓存、候选序列对应及取消后不确认旧候选。首轮性能与构建有重叠，且包含预热丢弃计数，保留target/annotation-capture-observation-performance.log但不用于性能结论。修正预热统计后独立运行三组完整30秒（target/annotation-capture-observation-final-performance.log），正式丢弃均0：four-overlaps 54.768次提交/秒、34.769ms P95；large-blur 50.336、35.558ms；long-brush 54.401、36.447ms。变化最多的源取帧间隔P95约32–33ms，比较确认观察P95约17–18ms；这些是worker观察，不是WGC真实到达或显示延迟。
- 检查发现WindowCapture新比较入队时comparison_waiting仍false；直到下一次try_complete返回Pending才置true。已在detector.begin后立即置true，复用原有Source 500us短重试，避免无变化基准先等待完整刷新周期。未增加Presentation轮询或帧队列。
- 扩展受控WGC原生回归：单次绘制后每个pending比较必须立即标记等待，并要求夹具确实观察到比较。该项通过（target/annotation-source-enqueue-wait-capture.log），另外12项原生GPU回归通过（target/annotation-source-enqueue-wait-native.log）。
- 修正后独立运行三组完整30秒Release（target/annotation-source-enqueue-wait-performance.log），正式丢弃均0：four-overlaps 60.099次提交/秒、22.506ms P95、GPU完整处理5.181ms P95；large-blur 60.867、29.507ms、3.908ms；long-brush 61.402、27.057ms、3.530ms。吞吐与尾间隔改善，但20ms门槛未通过；所有可见采集窗口都能引起提交，计数超过60不能证明独立60Hz源帧实际显示更快，输入到显示仍未测。
- 当前workspace 432 passed / 38 ignored（target/annotation-source-enqueue-wait-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-source-enqueue-wait-clippy.log）；Windows workspace Release通过（target/annotation-source-enqueue-wait-release.log）。
- 后续明确检查：presented_inputs相等、输出比较相等/成功提交/表面等待分支是否保留尚未完成的采集确认；补真实帧时间戳与夹具源身份，区分供帧和CPU观察等待后再改调度。真实App/DPI/IME、失败降级画面与剩余UI/场景职责继续未完成；未暂存、提交或推送。

## 采集等待传递、夹具源身份与生产节拍（2026-10-03）

- resolve_output显式接收当前采集确认状态；参数元数据相等、输出像素相等、成功提交和显示许可等待均保留Source等待。暂停仍保持Paused；候选接受、显示债务和基准授权规则不变。扩展原生单次源回归验证跨场景版本的参数相等以及成功提交不会消除仍在进行的采集确认。12项原生GPU回归通过（target/annotation-source-wait-propagation-native.log）。
- 测试诊断新增WGC SystemRelativeTime帧时间戳与夹具源HWND匹配，仅输出是否为夹具源及源序号，不输出其他窗口标题或HWND。时间戳间隔是被取到帧的WGC时间差，不等同于FrameArrived事件时间，也不是物理显示延迟。
- 等待传递修正后三组30秒（target/annotation-source-wait-propagation-performance.log）总提交62.802/62.200/62.236次每秒，提交间隔P95 32.036/32.110/32.128ms；夹具源均约48次取帧/秒，时间戳间隔P95约20.84ms，确认观察P95分别8.374/6.234/4.546ms。确认变快，但提交尾间隔未改善；总提交还包含其他窗口变化，不能据此证明目标源57FPS达标。
- 实机默认WGC MinUpdateInterval为160000个100ns单位，即16ms（target/annotation-wgc-default-interval.log）。CIM当前显示输出报告NVIDIA约143Hz、Oray虚拟显示60Hz；默认间隔在高刷新输出上量化为约三个刷新周期是与48FPS观测吻合的推断，不将其当作所有尾延迟的已证根因。[Microsoft属性定义](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.minupdateinterval?view=winrt-28000)说明该能力属于较新的WGC接口；运行时先检查getter，支持时设为0，setter失败保留原行为并只在窗口创建时记录debug，旧接口不禁用采集。
- 当前原生回归读回间隔0（target/annotation-wgc-producer-interval-capture.log），原生GPU12项通过（target/annotation-wgc-producer-interval-native.log）。取消生产节拍下限后的三组独立30秒Release（target/annotation-wgc-producer-interval-performance.log）：

| 场景 | 总提交/秒（不是目标源显示FPS） | 提交间隔P95 | 夹具源取帧/秒 | 夹具WGC时间戳间隔P95 | GPU完整处理P95 |
|---|---:|---:|---:|---:|---:|
| four-overlaps | 72.635 | 25.612ms | 约59.8 | 27.783ms | 4.219ms |
| large-blur | 65.399 | 32.652ms | 约51.3 | 34.729ms | 2.885ms |
| long-brush | 60.599 | 32.643ms | 约47.2 | 34.729ms | 3.253ms |

- 正式观测丢弃均0。只有重叠组供帧恢复近60；另外两组未达到目标源57FPS，全部提交间隔仍超过20ms。不宣称取消下限解决整体性能，也不以额外窗口提交填补目标源帧。
- workspace 432 passed / 38 ignored（target/annotation-wgc-producer-interval-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-wgc-producer-interval-clippy.log）；Windows workspace Release通过（target/annotation-wgc-producer-interval-release.log）；git diff --check通过。
- 下一批接入实际FrameArrived的有界通知与最新帧唤醒，区分到达/取帧/确认/输出，避免以整刷新期限轮询生产源；同时识别夹具绘制帧与被显示候选，测量独立源有效更新及输入显示延迟。UI所有权、编辑副本、失败降级及真实App/DPI/IME验收仍未完成；未暂存、提交或推送。

## 实际新帧通知与最新帧唤醒（2026-10-03）

- 新增Platform内部screen_capture::arrival所有者。WGC FrameArrived仅将原子pending置位，false→true时唤醒现有kernel signal及条件变量；多源/连续通知合并为一项，不排入帧任务、不访问GPU、不持有Mailbox锁。工作线程读取当前Backend自己的通知后提前进行一次最新帧尝试，保留正常刷新期限与比较短重试。
- 每个WindowCapture拥有Subscription，关闭时先禁用回调并RemoveFrameArrived，再关闭session/pool。回调只持有弱Notice，Notice的条件变量接线只持有弱Mailbox；不延长Renderer、窗口或会话生命周期。重建Backend创建独立通知，旧回调即使唤醒共享kernel signal也不能标记新Backend。软件采集Backend默认不注册通知，沿用原有30Hz采样。
- 测试诊断新增源通知到达时间，最多4096项；正式Mailbox仍最多131072项。通知时间是回调被执行的CPU时间，不等同于物理屏幕变化时间或SystemRelativeTime。源序列范围只统计实际取帧，不受通知事件的sequence=0干扰。
- 合并/弱引用/替换隔离/有界诊断回归通过：10次通知只唤醒一次，消费后允许下一次，4211条诊断仅保留4096并报告115条丢弃。受控WGC回归验证真实源唤醒后台，停止采集后不再收到通知（target/annotation-source-arrival-capture.log）；12项原生GPU回归通过（target/annotation-source-arrival-native.log）。
- 三组独立完整30秒Release（target/annotation-source-arrival-performance.log）：

| 场景 | 总提交/秒（不是独立源显示FPS） | 提交间隔P95 | 夹具源通知/取帧数 | 通知到达间隔P95 | 取帧间隔P95 | 确认观察P95 | GPU完整处理P95 |
|---|---:|---:|---:|---:|---:|---:|---:|
| four-overlaps | 75.133 | 21.991ms | 1825/1825 | 21.442ms | 21.622ms | 3.968ms | 3.991ms |
| large-blur | 75.366 | 22.082ms | 1828/1828 | 26.850ms | 26.532ms | 3.848ms | 3.073ms |
| long-brush | 75.166 | 22.090ms | 1825/1825 | 27.136ms | 27.227ms | 3.844ms | 2.910ms |

- 正式诊断丢弃均0。夹具源取帧均约60.8/秒，不再出现上批大模糊51.3、长笔刷47.2的低供帧；取帧间隔和确认观察改善。但20ms提交间隔门槛未通过，通知/取帧超过绘制60Hz也可能包含中途更新，不证明独立绘制帧或实际显示达到目标。SystemRelativeTime间隔P95为20.838/27.783/27.783ms，源节拍与处理延迟必须分别记录。
- workspace 433 passed / 38 ignored（target/annotation-source-arrival-workspace.log），含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-source-arrival-clippy.log）；Windows workspace Release通过（target/annotation-source-arrival-release.log）；git diff --check通过。
- 下一批建立不可变输出候选与独立源帧的对应，并核对夹具绘制是否产生中途更新；继续测输入到真实显示，不能用取帧或总提交代替。整体UI所有权、编辑副本、降级保留及真实App/DPI/IME需求未完成；未暂存、提交或推送。

## 参数到样式的映射脱离Registry（2026-10-03）

- 将style、text、watermark、sequence_style及endpoint映射迁入annotation_parameters。输入为AnnotationValues、已解析字体名称或字体目录及必要对象；没有Registry、Session、窗口或延迟任务依赖。协调入口仅负责取当前参数、语言字体选择和调用既有序号layout，映射不再承担排版。
- 保持现有颜色、描边、背景、字号下限、序号说明及水印转换规则；不调整字体回退策略、文本坐标或焦点。移除迁移后未使用的palette导入及parse_hex_color转发入口，测试使用的TextOutline/TextBackground改为测试导入。
- 首次迁移脚本因未显式指定UTF-8而在读取时失败，没有执行迁移；随后显式UTF-8重新执行。迁移后的Clippy发现两处遗留导入和无调用转发函数，已清理并重新验证，失败日志保留，不当作成功证据。
- 当前187项UI测试通过、1项忽略（target/annotation-parameter-style-mapping-verified-ui.log），覆盖既有序号说明、自定义颜色、字体、参数事务及场景绘制回归，含UI构建；workspace/all-targets Clippy -D warnings通过（target/annotation-parameter-style-mapping-verified-clippy.log）；Windows workspace Release通过（target/annotation-parameter-style-mapping-verified-release.log）；git diff --check通过。
- Registry入口当前7336行，包括测试。行数变化不是完成依据：默认值与对象投影仍共用values存储，菜单构建、编辑投影和协调任务也未完全解耦。下一批建立默认值与对象投影各自的状态来源及目标校验，避免用Index/IndexMut包装器隐藏原有写入；GPU独立源帧对应、输入显示、降级与真实App/DPI/IME验收继续未完成。未暂存、提交或推送。

## 参数默认值与对象投影所有权、P0回归修复（2026-10-03）

- annotation_parameters::State分别拥有逐工具defaults和按ObjectId校验的Projection；Registry通过明确values/replace接口接线，未增加Index/IndexMut兼容层。对象回填借用Core对象，不复制笔刷轨迹；矩形/椭圆两工具槽继续兼容Core共用工具身份。用户参数事件记录原对象身份，回调执行时拒绝已改变的目标；参数只更新自身负责的选中对象，其他工具的默认值不会修改无关对象。
- 新序号读取绘制默认参数和独立Core编号计数器，不继承当前选中序号的颜色、格式和大小。显式手动字体选择继续更新进程内偏好和后续绘制字体；对象回填不进入手动偏好路径。取消放置及对象手势、撤销后按恢复的对象重新生成投影；删除、选择目标变化和画布取消后同步面板，而非仅修改binding。无选择时恢复适用的语言字体默认值，现有编辑/水印保护规则保留。
- 前批accepted-workspace未通过：UI188通过/1失败/1忽略，序号测试期望新对象编号“八”，实际“8”（target/annotation-parameter-ownership-accepted-workspace.log）。该流水线未进入accepted-Clippy/Release；不将不存在的日志作为通过证据。复核确认该旧预期要求对象参数传播到绘制默认值，与已确认参数来源规则冲突；保留已选中对象中文格式、字号、说明比例、选区及继续输入断言，并增加新对象默认格式/字号、删除后面板回填、旧对象事件拒绝、新目标实际样式回填断言。
- 新增defaults/Projection隔离及矩形/椭圆别名回归；新序号默认参数与取消恢复选中对象/编号的回归通过。最初新序号测试夹具零边界导致Session.add失败，已通过现有layout构造合法对象；本批扩展删除断言初次误用delete_selected的单元返回类型，编译失败后修正。初次Clippy报告冗余style:style字段，改为简写并重新验证。失败日志保留，不当作最终证据。
- 最终workspace435 passed / 38 ignored（target/annotation-parameter-baseline-workspace.log），含UI189 passed / 1 ignored及Slint编译。workspace/all-targets Clippy -D warnings通过（target/annotation-parameter-baseline-fixed-clippy.log）；Windows workspace Release通过，2m13s（target/annotation-parameter-baseline-release.log）；git diff --check通过。未更改GPU链路，未重复性能测试。
- Registry入口当前7528行，包含测试；本批收益是状态来源和失效目标闭环，不是缩短文件。基础defaults/对象Projection分离不代表完整参数批次完成：空白编辑草稿、水印独立状态、延迟事件的编辑实例校验、菜单请求、工作场景复制、故障降级和真实App/DPI/IME仍待继续。原生GPU独立源/输出候选/真实显示对应及完整性能目标不变。未暂存、提交或推送。

## 参数值事件捕获编辑实例来源（2026-10-03）

- Registry.parameter_target生成包含面板版本、工具、ObjectId和文字编辑实例的现有参数Target；on_values_changed在事件发出时捕获整份Target，update_geometry_values执行时整体比较。旧实现只捕获ObjectId，到执行时才读取editor编号；同一对象关闭重开后，旧事件可能被赋予新实例编号。现已拒绝此类事件，不修改新编辑、对象参数、默认值或撤销事务。
- annotation_editor::State拥有Rc<Cell<Option<实例编号>>>身份句柄，提供给Registry重入期间的参数回调。open发布新编号；take、clear及Drop先使身份失效，再停止计时器和释放组件。句柄不持有输入窗口、Renderer或编辑草稿，不延长窗口生命周期；正常用户事件仍优先读取当前Registry目标。
- 扩展真实面板回调回归：Registry借用期间向旧序号说明实例排入70号/Roman样式事件，关闭后立即对同一ObjectId重开，执行定时事件后仍保留28号/中文序号和52.5号说明。原有选区保留、直接继续输入、删除后过期对象事件及新对象默认值断言均保留。编辑所有者回归验证身份句柄在打开、取出、替换、销毁后的变化及无窗口保留。
- 针对性回归通过（target/annotation-parameter-editor-source-ui.log）。workspace435 passed / 38 ignored，含UI189 passed / 1 ignored及Slint编译（target/annotation-parameter-editor-source-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-parameter-editor-source-clippy.log）；Windows workspace Release通过（target/annotation-parameter-editor-source-release.log）；git diff --check通过。没有GPU链路改动，不重复性能测试；受控回调验证不代表真实DPI/IME画面验收。
- 本批仅闭合参数值事件的编辑身份；空白草稿、水印独立参数来源、菜单/拖动结束/焦点等其余事件入口、编辑场景复制、故障降级及GPU独立源/候选/显示对应继续保留。Registry入口当前7569行（含测试），整体重构目标未完成。未暂存、提交或推送。

## 新文字草稿独立拥有参数（2026-10-03）

- annotation_editor::Instance为无ObjectId的新文字拥有独立AnnotationValues；已有文字与序号说明不保存该副本，继续使用对象Projection。Registry.values/replace_values仅在工具8且活动编辑是新草稿时路由到它。空白草稿角点缩放后的字号、描边和背景尺寸回填同样进入编辑所有者，不修改后续绘制默认值。
- ClosedEdit取出草稿参数，提交使用其Style创建Core文字对象，保留文字、字号、自定义颜色及背景内边距。取消释放草稿参数；准备失败走同一取消入口。编辑取出后先同步文字面板，提交后再按新对象回填，避免后续close_panel将旧草稿值误写为默认值。字体显式手动选择仍通过原有偏好路径保存，不改变进程内偏好规则。
- 新增真实面板参数回调回归：1.25创建DPI、负坐标草稿，修改58号、自定义RGB 16/32/48及13内边距，输入中文。取消后默认参数和撤销栈保持原值；提交后对象样式/文字完整保留，一次撤销移除新对象且无额外参数撤销。两条路径都在编辑结束后关闭工具条，验证不污染默认参数。原有普通文字、序号说明、焦点、对象身份、窗口关闭及失效范围回归保留。
- 初次新增测试误写不存在的text_bg字段，编译失败后按现有AnnotationValues修正为text_background，未改变Slint接口。针对性取消/提交回归通过（target/annotation-draft-parameter-fixed-cancel-commit.log）。最终workspace436 passed / 38 ignored，含UI190 passed / 1 ignored及Slint编译（target/annotation-draft-parameter-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-draft-parameter-clippy.log）；Windows workspace Release通过（target/annotation-draft-parameter-release.log）；git diff --check通过。未改GPU链路，未重跑性能测试；这些测试不替代真实App/DPI/IME截图。
- Registry入口当前7689行，包含测试。下一批处理水印独立参数来源；其余延迟事件、编辑场景副本、失败降级、独立源/候选/真实显示对应及完整App验收继续未完成。本批没有新增Core接口、crate、配置或依赖，未暂存、提交或推送。

## 已应用水印的独立参数投影（2026-10-03）

- annotation_parameters::State分别保存后续绘制defaults与已应用watermark投影。adopt_watermark_style使用借用Core水印回填投影；apply_watermark不再把模板写入defaults。工具9读取/编辑当前投影，未应用水印时使用默认值。对象取消选中不清除文档级水印投影，关闭会话仍销毁整个状态。默认字体语言更新不再因存在水印而跳过，但实际水印字体始终来自投影，不修改Core对象。
- 每次应用、清除、撤销或重新回填都更新UI内部投影版本；连续参数调整保留版本。参数Target增加该版本，事件发出时捕获、执行时校验，避免清除或撤销后的旧事件以ObjectId=None为由修改新默认值。重入回调读取只持有版本Cell的句柄，不持有窗口。Core接口、解析规则、窗口应用/取消和连续参数撤销机制不变。
- 扩展纯参数回归：回填、编辑和取消对象选中均不污染默认值，清除投影恢复默认值且旧版本失效。新增真实面板回调回归：应用“甲”，调整44号、自定义RGB 18/52/86、70不透明度及居中位置；打开输入写入未应用文本后取消，已应用模板不变；排入旧68号事件后应用空内容清除，事件被拒绝且默认值不变；撤销恢复44号及自定义颜色选中态。
- 针对性回归通过（target/annotation-watermark-projection-ui.log）。workspace437 passed / 38 ignored，含UI191 passed / 1 ignored及Slint编译（target/annotation-watermark-projection-workspace.log）。初次Clippy提示检查后unwrap，改为if let直接绑定；最终workspace/all-targets Clippy -D warnings通过（target/annotation-watermark-projection-fixed-clippy.log）。Windows workspace Release通过，2m13s（target/annotation-watermark-projection-release.log）；git diff --check通过。没有GPU链路变化，不重复性能测试；这些验证不替代真实DPI/IME截图。
- Registry入口当前7798行，包含测试。基础defaults/对象/新文字草稿/已应用水印来源已分别接线，但菜单请求及其他延迟入口仍访问较宽Registry上下文，编辑场景复制、故障降级、独立源/候选/显示对应和完整App验收继续未完成。整体目标保持，未新增crate、配置或依赖，未暂存、提交或推送。

## 三级菜单绑定打开时的参数目标（2026-10-03）

- 普通三级菜单保存打开时的参数Target，包含工具条实例、工具、ObjectId、文字编辑实例和水印投影版本。选择、颜色、文字样式、窗口显示重试及延迟返回父面板共用目标匹配；水印时间菜单继续使用独立输入窗口版本，不冒充普通参数菜单目标。
- 工具条位置、所属窗口或参数目标变化时销毁菜单及其准备/定位任务。文字编辑取出/重开、对象参数采用、水印重新回填与画布目标刷新接入此关闭入口；不销毁二级工具条。过期菜单关闭不执行新编辑的焦点恢复，也不将焦点切到工具条输入处理区；原目标仍有效的正常关闭保留现有输入恢复和撤销收尾。
- 扩展真实菜单回调回归：对序号说明打开字体菜单，排入选择事件后关闭旧编辑，立即对同一ObjectId重开。刻意持有旧菜单组件并再次触发字体选择，验证字体、字号、格式及说明实例不被修改，菜单已销毁且二级工具条保留；原插入选区和继续输入断言保留。独立菜单显示、时间菜单、颜色子面板返回、替换与资源释放等既有回归保留。
- 针对性重开回归通过（target/annotation-menu-target-reopen.log），相关annotation回归86通过（target/annotation-menu-target-ui.log）。最终workspace437 passed / 38 ignored，含UI191 passed / 1 ignored及Slint编译（target/annotation-menu-target-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-menu-target-clippy.log）；Windows workspace Release通过（target/annotation-menu-target-release.log）；git diff --check通过。没有GPU代码改动，不重复性能测试；受控组件回调不等于真实App/DPI/IME画面验收。
- Registry入口当前7841行，包含测试。本批闭合目标改变的菜单生命周期，但菜单构建仍使用较宽Registry上下文；普通对象在相同ObjectId下撤销后的投影版本、其他延迟事件、编辑场景副本、故障降级、独立源/候选/真实显示对应及完整App验收仍未完成。整体目标保持，未暂存、提交或推送。

## 普通对象投影版本与同身份撤销隔离（2026-10-03）

- annotation_parameters::State拥有独立投影版本。对象重新回填及取消选中更新版本，连续参数编辑不更新；水印继续使用自己的投影版本。参数Target包含该版本，普通工具值事件和菜单在执行时完整匹配；重入回调通过不持有窗口的Cell句柄捕获版本，不读取Core文档或复制轨迹。
- restore_parameter_projection先切换投影版本，再清理不匹配菜单和回填面板。撤销恢复相同ObjectId时也能拒绝旧值事件，避免把恢复的线宽、外形等覆盖回旧值。菜单关闭遵守既有焦点恢复规则，二级工具条保留。UI事件版本不参与文档签名、场景内容或GPU提交版本，不因版本变化增加栅格化或采集。
- 新增真实回调回归：创建负坐标矩形，将线宽改为11并结束参数事务；打开外形菜单，排入80线宽和椭圆选择；撤销到原始样式后再次触发刻意保留的旧菜单。验证对象身份不变、外形/线宽正确、旧菜单销毁、工具条仍显示；第二次撤销移除创建且无额外历史。纯状态回归补连续编辑版本稳定与新投影版本变更断言。
- 针对性回归通过（target/annotation-projection-epoch-undo.log）。workspace438 passed / 38 ignored，含UI192 passed / 1 ignored及Slint编译（target/annotation-projection-epoch-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-projection-epoch-clippy.log）；Windows workspace Release通过，2m13s（target/annotation-projection-epoch-release.log）；git diff --check通过。无GPU链路改动，未重复性能测试；受控组件回归不代替真实App/DPI/IME截图。
- Registry入口当前7960行，包含测试。下一批继续收窄菜单请求/参数应用接口及其他延迟入口；编辑场景副本、故障降级、独立源/候选/真实显示对应和完整App验收仍未完成。整体目标保持，未新增Core接口、crate、配置或依赖，未暂存、提交或推送。

## 系统QPC间隔及测量边界核算（2026-10-03）

- Surface复用已有DirectComposition统计中的timeFrequency并缓存到测试实例，无新增Windows功能或依赖。Observation保存频率；interval_ms仅接受同链路/同频率、正值单调QPC及PresentRefreshCount=SyncRefreshCount的前后样本。纯回归证明CPU查询相隔90ms但QPC应为20ms，且刷新计数不匹配、时钟频率变化、其他链路及非递增QPC均拒绝。
- 系统计数统计进一步只接受测量区间内取帧的来源版本，排除热身边界。Release三组RTX4060/30秒通过（target/annotation-display-qpc-native.log、各case.txt）：四重叠/大模糊/长笔刷确认1788/1785/1794个所属源版本，59.602/59.504/59.801每秒；频率均10MHz，有效间隔1787/1784/1793，时钟/刷新不匹配排除0，边界来源排除0/1/0。系统QPC间隔P95均27.784ms，仍高于20ms门槛。此值替代CPU观察时间作为本机系统间隔证据，不意味着优化加速，也不单独证明GPU计算是瓶颈。
- API频率依据：[DCOMPOSITION_FRAME_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/dcomptypes/ns-dcomptypes-dcomposition_frame_statistics)。仅使用其中QPC频率，没有把nextEstimatedFrameTime或合成批次时间当作实际显示时间。仍沿用DXGI统计的多屏及硬件队列限制；未验证光子时间、独立paint ID/像素及真实用户输入。
- workspace443通过/39忽略，含Platform94通过/36忽略、UI193通过/1忽略及Slint编译（target/annotation-display-qpc-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-display-qpc-clippy.log）；Windows workspace Release通过（target/annotation-display-qpc-release.log）；git diff --check通过。源/显示刷新周期与调度归因、输入延迟、失败降级、场景副本及UI剩余迁移保持完整范围。未暂存、提交或推送。

## 系统呈现编号关联冻结候选（2026-10-03）

- 新cfg(test) display_timing模块按交换链实例和提交编号索引冻结来源。Surface实例编号不复用；成功原生提交后读取GetLastPresentCount，系统GetFrameStatistics观察保存PresentCount及原始QPC/刷新计数。精确匹配报告编号，跳过编号不确认中间候选；重复统计不重复计数，其他实例不可匹配。纯回归验证跳号、重复和其他链路。
- Worker只在测试记录期查询系统统计，频率限制2ms，调用在Mailbox锁外、GPU测量结束后；记录上限16384，错误和丢弃单独计数。未改生产提交/等待逻辑，不引入DwmFlush、像素读回或Core接口。该查询仍可能影响测试CPU时序，不能把有/无诊断的细微差异归因于生产算法优化。
- 三组RTX4060/30秒原生测试通过（target/annotation-display-link-native.log、各case.txt）。四重叠/大模糊/长笔刷统计记录7543/7444/7702，精确匹配2064/2133/2175次Native提交，所属源不同版本1772/1790/1794，折算59.067/59.669/59.802每秒。记录丢弃0，每组一次统计错误；PresentRefreshCount与SyncRefreshCount不一致数0。所属源CPU观察间隔P95 29.138/29.440/28.963ms，不是精确系统呈现间隔，下一步需要QPC频率核算。
- 系统计数仍非光子时间或源paint ID/像素证据；当前计数尚未进一步排除测量边界来源版本，报告保留该限制。没有宣称完整性能门槛、输入到显示或真实App/DPI/IME验收达成。原系统API多屏可靠性限制同上一节，后续需覆盖统计失效/滞后及窗口/设备重建。
- workspace442通过/39忽略，含Platform93通过/36忽略、UI193通过/1忽略及Slint编译（target/annotation-display-link-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-display-link-clippy.log）；Windows workspace Release通过（target/annotation-display-link-release.log）；git diff --check通过。整体参数/菜单剩余迁移、编辑场景副本及失败降级范围保持。未新增crate、依赖或配置，未暂存、提交或推送。

## 首次提交口径与显示统计能力探测（2026-10-03）

- owned_commits纯函数为区间内取帧建立sequence/ticks索引，仅计该版本首次成功提交；热身帧、未匹配版本、混合候选及重复提交不进入所属源FPS与首次提交延迟。新增纯回归包含热身帧、同版本重复提交和混合候选，断言只剩一个版本及4ms延迟（target/annotation-display-statistics-boundary.log）。避免旧统计把重复提交的旧源年龄作为首次显示延迟。
- 三组RTX4060/30秒Release测量通过（target/annotation-display-statistics-performance.log及各case.txt）：四重叠/大模糊/长笔刷1798/1800/1800个所属源版本首次提交，59.935/60.001/60.003每秒，间隔P95 25.847/24.461/24.354ms；首次取帧到提交P95 9.925/9.388/9.220ms。分别排除445/447/448个重复提交及1/2/0个未匹配记录，混合候选0。此前约22ms为含重复提交口径，不可解释为本批使渲染算法加速；区间P95门槛仍未达成，物理显示及paint ID尚未核验。
- Surface增加cfg(test)独立统计探测，调用GetLastPresentCount、GetFrameStatistics和GetFrameStatisticsMedia，不改变生产present，不在性能测量中增加这些查询或DwmFlush。30次非激活所属窗口提交的原生探测通过（target/annotation-display-statistics-native.log、annotation-display-statistics-probe.txt）；当前交换链返回推进的呈现计数和QPC，首帧DXGI_ERROR_FRAME_STATISTICS_DISJOINT、部分轮次计数滞后。探测不是显示延迟验收，后续需处理统计失效/滞后、对应冻结候选及多屏限制。
- API依据：[GetFrameStatistics](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-getframestatistics)、[DXGI_FRAME_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics)。文档限定flip/全屏链路，并指出多屏可靠性及硬件flip queue更新限制；不把成功返回的SyncQPCTime直接当作光子时间。首次探测测试补齐现有RenderSurface.scale字段后编译通过，未改Port。
- workspace441通过/39忽略，含Platform92通过/36忽略、UI193通过/1忽略与Slint编译（target/annotation-display-statistics-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-display-statistics-clippy.log）；Windows workspace Release通过（target/annotation-display-statistics-release.log）；git diff --check通过。整体UI迁移、场景复制、失败降级及实际App/DPI/IME/显示验证仍保留，未暂存、提交或推送。

## 采集纹理来源跟随冻结候选和成功提交（2026-10-03）

- Source诊断区分最新取帧、比较候选和已发布纹理版本，保存token/sequence/WGC ticks。初始/direct copy发布当前帧；比较Changed发布冻结候选，Equal保持旧纹理来源；ROI扩展若发布仍保留的候选，也更新来源。取消不发布。新增纯回归在候选后引入新帧，证明不会用最新帧冒充冻结纹理；保留有界观察及候选确认测试。
- cfg(test)的CaptureLayer携带来源，Effects仅在真正重算效果时冻结采样范围内来源及下层效果依赖。缓存、暂停和未重算效果保留旧来源；删除资产清理对应记录。PresentationInputs保存快照，来源不参与生产相等比较。Renderer只在成功原生提交后记录该候选来源，不从已经前进的Effects重新取值；记录有16384上限及丢弃计数。
- 扩展既有原生失败/等待回归，为输入纹理赋予可区分版本；比较期间更新新源后，成功提交记录严格等于冻结候选，且与最新效果来源不同。Release回归通过（target/annotation-source-provenance-candidate.log）。编译中修正事件Vec被消费后再使用及测试命名空间问题，没有放宽行为断言。
- RTX4060三组30秒原生测量全部通过（target/annotation-source-provenance-native.log及各case报告）：四重叠/大模糊/长笔刷所属源不同提交版本分别1798/1800/1801，折算59.935/60.003/60.036每秒；间隔P95 25.826/24.643/24.878ms；取帧到提交P95 22.295/22.310/22.207ms。混合版本及来源记录丢弃均为0；每组1个提交无法匹配区间内取帧记录，因此计数包含区间边界，不宣称逐个paint ID完整对应。诊断跟踪的是纹理版本，不校验像素或实际物理显示；effective_display_fps及input_to_visible继续未测，完整性能门槛未达到。
- workspace440通过/38忽略，含Platform91通过/35忽略、UI193通过/1忽略和Slint编译（target/annotation-source-provenance-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-source-provenance-clippy.log）；Windows workspace Release通过（target/annotation-source-provenance-release.log）；git diff --check通过。剩余区间边界统计、源paint ID/像素验证、实际显示、故障降级、UI剩余迁移和真实App/DPI/IME验收保持原范围。未暂存、提交或推送。

## 实时测试源一次发布完整帧（2026-10-03）

- 同机重跑原逐色带大模糊基线：30秒绘制1800帧，所属窗口取帧1824次，总提交74.997/s，提交间隔P95 22.128ms（target/annotation-source-publish-baseline.log及对应baseline-large-blur.txt）。并非每个取帧/提交都可认定为新的完整源帧。
- cfg(test)性能夹具SourceFrame复用离屏兼容位图/DC，在内存DC画完8条色带后一次BitBlt写入窗口并GdiFlush。初始化检查所属窗口各色带的顶部/底部颜色；计时期间不额外读回像素。RAII在销毁时恢复原位图、删除位图/DC及释放窗口DC，失败分支也由该所有者清理。该初始化检查不证明WGC帧或屏幕显示的完整性，后续仍需版本对应；未改生产采集、效果算法或显示调度。
- Release原生三组各30秒、RTX4060全部通过（target/annotation-source-publish-native.log）。四重叠/大模糊/长笔刷各绘制1800帧、所属源各取帧1800次；总提交分别74.801/75.370/75.070每秒，提交间隔P95分别21.908/22.171/21.953ms。其余窗口仍有取帧，故总提交不能代表所属源的有效显示FPS。本批消除夹具逐色带发布的额外取帧，未证明用户体验延迟下降。
- 三组报告以annotation-source-publish-{case}.txt保存，新增source_publication、source_to_candidate=not_measured、effective_display_fps=not_measured。有效源版本→不可变候选→成功提交，以及实际显示/输入延迟仍缺少完整证据，性能门槛尚未达成。
- workspace439通过/38忽略，含UI193通过/1忽略及Slint编译（target/annotation-source-publish-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-source-publish-clippy.log）；Windows workspace Release通过（target/annotation-source-publish-release.log，本批仅改测试代码，生产构建缓存有效）；git diff --check通过。无真实App/DPI/IME交互截图。整体重构继续，未暂存、提交或推送。

## 马赛克手势模式与对象参数的字段角色（2026-10-03）

- 扩展实际面板回调回归：选中区域、切换笔刷并修改模糊/强度，取消选中后应保留笔刷而恢复默认效果及50强度。旧实现复现为MosaicArea（target/annotation-mosaic-mode-baseline.log）。原因是整份参数替换把手势模式仅写入对象Projection，取消选中即丢失。
- State记录UI内部显式mosaic_mode；replace只在模式字段确实变化时保存选择及默认shape，其他参数仍更新原目标。对象回填没有显式选择时保持对象类型；有选择时保留手势模式，不转换对象轨迹。新增纯状态回归覆盖选中旧笔刷的直径入口、调整强度不污染默认值、主动切换区域/笔刷、重新回填和取消选中。初版直接用默认模式覆盖所有回填，被该回归证实会禁用旧笔刷控件（target/annotation-mosaic-mode-refill-baseline.log），已改为仅保留显式选择；没有放宽断言。
- 最终马赛克相关UI回归19通过（target/annotation-mosaic-mode-final-ui.log）；workspace439通过/38忽略，含UI193通过/1忽略及Slint编译（target/annotation-mosaic-mode-final-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-mosaic-mode-final-clippy.log）；Windows workspace Release通过（target/annotation-mosaic-mode-final-release.log）；git diff --check通过。首轮通过记录仅对应中间实现，最终记录覆盖修正后的字段角色。
- GPU链路未改动，未重新测量延迟，未保存真实App/DPI/IME交互截图；受控组件回调不替代该验收。下一批转向GPU独立源帧完整性及实际显示对应，菜单剩余接线、编辑场景副本、失败降级与完整App验收仍未完成。未新增Core接口、配置、crate或依赖，未暂存、提交或推送。

## 菜单构建使用明确展示输入（2026-10-03）

- build_window只接收展示请求、工具类型和不可变AnnotationValues，不访问Registry、工具条组件、文档事务或编辑焦点。协调层从defaults/对象/草稿/水印的有效参数来源取得快照；构建后再绑定带目标校验的回调，菜单关闭、返回父面板及原生展示生命周期不变。时间菜单与其余回调协调尚未全部迁移，不把这个函数提取视为完整菜单解耦。
- 无Registry的受控窗口回归验证参数快照、序号外形及描边启用条件；原展示服务、负坐标定位、准备重试及资源释放断言保留。首个测试筛选未匹配用例，已纠正为实际9项菜单相关回归。新增测试字段最初误用名称/类型，编译指出后按现有AnnotationValues修正，未改Slint接口。
- 初次workspace发现颜色面板夹具仅修改Slint显示值，没有写入实际参数来源；改为同步设置来源，保留两种DPI下自定义按钮点击、色板居中、子面板返回及延迟取消断言。对应回归通过（target/annotation-menu-builder-color.log）。最终workspace438通过/38忽略，含UI192通过/1忽略及Slint编译（target/annotation-menu-builder-fixed-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-menu-builder-clippy.log）；Windows workspace Release通过（target/annotation-menu-builder-release.log）；git diff --check通过。
- 本批未改GPU链路，没有重新测量实时性能；受控回调不替代真实App/DPI/IME截图。菜单协调入口、编辑场景副本、失败降级、独立源/候选/实际显示对应及完整App验收仍未完成。整体目标保持，未暂存、提交或推送。

## 菜单补丁与共用参数入口的原子应用（2026-10-03）

- 先补回归复现：向序号说明接受RGB 16/32/48菜单补丁后立即结束编辑，旧实现仅更新Slint工具条，实际参数事件尚未应用，结果仍为原RGB 217/71/43（target/annotation-menu-atomic-baseline.log）。该失败明确证明属性回调不能承担已接受菜单命令的二次转发。
- 将update_geometry_values内部应用逻辑提取为共用apply_geometry_values，目标校验、默认值/投影/草稿写入、Core参数更新、300ms撤销收尾及窗口定位保持同一实现。工具条外部事件按会话解析Registry后调用；菜单携带打开时的Target及明确Edit字段补丁，基于当前参数合并并直接调用，不读取整份旧工具条快照，也不等待Slint回调完成命令。参数应用后回填工具条，重复属性事件因值相同而不产生事务。
- 移除马赛克菜单Choice的特殊invoke_values_changed分支；菜单颜色、样式、字体和普通选项共用目标/补丁入口。手动字体偏好保持原规则，仅在目标有效时更新。菜单应用完成后只延迟渲染请求，以避开Registry重入；对象与编辑草稿已同步更新，关闭编辑不会丢失已接受的样式。
- 针对性回归修复后通过（target/annotation-menu-atomic-fixed.log），保留原字号、说明比例、选区、直接输入、对象/编辑版本与默认值隔离断言，增加立即结束后的说明颜色断言。workspace438 passed / 38 ignored，含UI192 passed / 1 ignored及Slint编译（target/annotation-menu-atomic-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-menu-atomic-clippy.log）；Windows workspace Release通过（target/annotation-menu-atomic-release.log）；git diff --check通过。无GPU链路改动，未重跑性能测试；这些回归不代替真实App/DPI/IME截图。
- Registry入口当前7987行，包含测试。菜单窗口构建仍需改为独立请求/能力输入，其他延迟入口、编辑场景副本、故障降级、独立源/候选/显示对应与完整App验收继续未完成。整体目标不变，未新增Core接口、crate、配置或依赖，未暂存、提交或推送。

## 采集比较实验撤回与刷新周期诊断（2026-10-03）

- 上一轮绕过采集变化比较、仅保留最终输出比较的实验，两次大模糊测量结果不稳定：首次系统计数55.968版本/秒、QPC间隔P95 34.729ms，复测59.133版本/秒、27.783ms。GPU P95约1.8ms及首次取帧到提交改善不能替代显示门槛。日志保留在target/annotation-latest-source-experiment.log、annotation-latest-source-repeat.log；已仅撤回sample_gpu_latest包装及两处渲染调用，恢复采集比较，不覆盖其他工作区改动。
- 新增仅测试使用的refresh_interval诊断：沿用QPC/链路/刷新计数一致性校验，以呈现刷新计数差计算跨周期数及推算周期；零差、倒退、回绕和不一致样本排除。纯回归覆盖这些边界，不查询或改变系统刷新率。
- 当前Release大模糊原生夹具运行通过（target/annotation-refresh-cadence-native.log）：系统计数59.102版本/秒，GPU P95 3.175ms，首次取帧到提交P95 10.421ms，QPC间隔P95 27.784ms。1772个间隔的周期直方图为{1:354, 2:593, 3:570, 4:225, 5:21, 6:8, 8:1}；无无效周期样本，推算周期中位6.946ms。这是OS元数据的推断，不是显示器模式读取、像素ID或物理显示证明，不能据此宣称具体调度原因。20ms门槛仍未通过，未降低标准。
- Platform lib94通过/36忽略；workspace444通过/39忽略（target/annotation-refresh-cadence-tests.log、annotation-refresh-cadence-workspace.log），包含Slint编译。workspace/all-targets Clippy -D warnings通过（target/annotation-refresh-cadence-clippy.log）。Windows workspace Release通过（target/annotation-refresh-cadence-release.log）；git diff --check通过。
- 本批未改变文字、菜单或普通标注行为，未保存真实App/DPI/IME截图。下一步仍需排查等待/呈现调度，验证源像素版本、输入延迟、故障降级及剩余UI所有权迁移；完整目标保持。未暂存、提交或推送。

## 水印与参数菜单共用列表构建和选择事件（2026-10-03）

- 水印时间菜单不再独立构建Slint窗口和重复绑定选择/取消/原生关闭回调。build_selection_window只接收展示请求，负责主题与列表属性；普通参数菜单复用它再增加参数控件。bind_selection只向调用方提供的接收器报告Option<int>，窗口层不读取Registry、应用参数或恢复焦点。协调层仍校验会话、所属实例和菜单版本并处理延迟收尾，没有引入新的Core接口或依赖。
- 扩展真实水印菜单回调回归：保留退休菜单强引用，在输入窗口替换前排入选择事件，再在重开后触发旧选择和取消，确认新窗口仍为空、选中Slash 24h且菜单保持打开。随后操作当前菜单选择ISO，确认模板追加、标签选中变化、下一帧菜单关闭及直接键入中文继续输入。target/annotation-menu-selection-watermark-final.log通过；受控Slint事件不等同于实际系统焦点/IME验证。
- 本批仅调整Rust菜单构建/回调边界与回归；未改Slint外观或GPU链路，未重测GPU性能。完整菜单协调、窗口任务所有权、真实App/DPI/IME、场景副本、失败降级及显示性能门槛仍未全部完成。workspace444通过/39忽略，包含Slint编译（target/annotation-menu-selection-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-menu-selection-clippy.log）；Windows workspace Release通过（target/annotation-menu-selection-release.log）；git diff --check通过。未暂存、提交或推送。

## 原生编辑场景借用未编辑对象（2026-10-03）

- UI SceneCompiler增加仅UI内部的Document借用视图：沿用Core Session及对象顺序，仅替换活动编辑对象或在末尾追加新文字。原生render_canvases不再先构建复制所有objects的visible_session；只有转入软件呈现/效果路径时才创建旧式拥有快照。序号说明的替换保留徽章对象身份及样式，仅替换说明文字/边界；普通文字和新草稿沿用相同edited_document_object组合规则。
- 原生交互缓存按视图解析实际选中/悬停对象，绘制控制点时只组装这两个对象，不需要全部文档。文档和交互覆盖层仍分开，异步后端接收已编译的不可变资源，未把UI借用传到后台线程或Core公开接口。
- 新回归覆盖普通文字/序号说明、替换/追加、100%/125% DPI，以旧拥有预览为画面对照，最大通道误差≤1；含20000点笔迹的未编辑对象及points缓冲区地址保持与Core来源相同，文档数量/撤销能力保持。夹具最初未布局徽章导致零边界对象被Core拒绝，已补真实布局及add成功断言，没有放宽借用/画质断言。target/annotation-borrowed-edit-regression-final.log通过；原场景8项回归通过。首轮Clippy指出两处可合并if，已修正，无lint豁免。
- 软件效果快照及下层worker副本仍待迁移；本批不宣称已消除所有编辑副本，也未测量实际输入延迟、GPU显示性能或保存实机截图。最终workspace445通过/39忽略（target/annotation-borrowed-edit-final-workspace.log），包含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-borrowed-edit-final-clippy.log）；Windows workspace Release通过（target/annotation-borrowed-edit-final-release.log）；git diff --check通过。完整重构继续，未暂存、提交或推送。

## 软件效果下层与放大镜取景借用对象范围（2026-10-03）

- 核对代码后，原软件任务的lower Session逐个深复制对象一次，不是每个效果重建全部前缀；放大镜取景则另行复制整组对象并把被排除对象改成空矩形。现移除这两处拥有副本：马赛克从当前Job的不可变Session借用到效果之前的切片，放大镜使用同一对象范围和取景策略排除其他镜头/后续效果，遍历先编号再排除，保留Tile原索引。必要跨线程Job快照仍保留，不把Core或UI借用带到后台。
- LayerScope明确对象范围、是否绘制交互层及镜头所在索引；后台采样和镜头内部关闭选框绘制，背景聚光灯也只读取当前范围。水印、显示区域及聚光不透明度从Job元数据借用。普通画布继续绘制交互，镜头内部对普通标注的既有取景规则不变。
- 原渲染相关36通过/1忽略（target/annotation-borrowed-lower-render.log），含跨DPI下层合成、后续标注保持清晰、放大镜无递归、缺失采集保留、分块笔刷与模糊边缘。新增采样前缀回归在效果之后增加覆盖取景区的聚光背景，并分别选择前层/效果/后层；完成tile的索引、边界和像素必须与无选中基线完全相同（target/annotation-borrowed-lower-prefix.log通过），防止借用完整Session误把选框或后层背景加入采样。
- 本批未改GPU调度，未测量输入/显示延迟收益，也未保存真实App截图。UI软件编辑投影及必要Job快照仍待后续梳理，完整性能/资源与真实交互验收范围保留。workspace446通过/39忽略（target/annotation-borrowed-lower-workspace.log），包含Slint编译；workspace/all-targets Clippy -D warnings通过（target/annotation-borrowed-lower-clippy.log）；Windows workspace Release通过（target/annotation-borrowed-lower-release.log）；git diff --check通过。未暂存、提交或推送。

## 软件任务共享不可变文档快照（2026-10-03）

- 软件Job的文档改为Arc<Session>，UI软件链路仅缓存最新一份快照。新采集serial仍按现有提交/取消协议处理，但完整文档、草稿及背景元数据相同则复用Arc和轨迹缓冲区。内容变化创建新快照，不修改旧任务持有的数据；必要跨线程拥有边界保留，不把Core对象借用跨线程。缓存采用完整对象/草稿、水印、displays和spotlight_opacity精确比较，不以裁剪后的视口key作为完整身份；选中状态不进入效果采样快照。该比较仍扫描文档，未声称已消除全部热路径成本。
- stop_software和采集区域清空均释放缓存；正在执行/待处理任务各自拥有其快照，结束或取消后按Arc所有权释放。现有latest-only mailbox及按context取消规则未改变。测试夹具使用显式唯一所有权的session_mut；首轮编译暴露另一处Job构建仍用Session及闭包借用冲突，已改为Arc和独立Rect捕获，没有新增Core接口或依赖。
- 相关马赛克回归38通过/1忽略（target/annotation-shared-snapshot-mosaic-fixed.log）。新增20000点共享缓存回归验证Arc及points地址复用、选择不失效、屏外点/显示区域/聚光参数/水印/草稿变化生成新快照、取消去除草稿、旧任务快照冻结与clear释放；停止软件链路回归增加Weak释放断言（target/annotation-shared-snapshot-cache.log及annotation-shared-snapshot-stop.log通过；水印断言随后加入workspace门禁）。
- UI软件编辑visible_session的拥有副本尚未迁移，GPU显示间隔门槛、实际输入延迟与真实App/DPI/IME及资源验收继续保留。本批未改变GPU调度，没有重测显示性能，不以结构优化推断帧率收益。workspace447通过/39忽略，包含Slint编译（target/annotation-shared-snapshot-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-shared-snapshot-clippy.log）；Windows workspace Release通过（target/annotation-shared-snapshot-release.log）；git diff --check通过。未暂存、提交或推送。

## 软件编辑绘制共用借用文档视图（2026-10-03）

- 删除visible_session以及render_canvases在软件路径构建全部objects副本的步骤。原生、缓存软件、直接软件效果、渲染签名、采样签名、手势上下文及共享后台快照均读取同一UI Document视图。编辑对象单独拥有，未编辑对象借用；同步捕获可能移除临时对象后重建编辑替换，再进入软件呈现，保持原有失败收尾规则。
- LayerScope从连续对象切片改为Document加范围上限，借用迭代保持原全局索引；放大镜揭示早期效果及取景排除规则仍相同。软件控制点绘制直接解析视图选中/悬停引用，不需要拥有Session副本。SceneCompiler缓存软件入口也接收编辑替换；后台Snapshot只在内容变化时物化同一视图，返回拥有的Arc后才跨线程。
- render_canvases借用拖动Preview，移除额外复制长笔刷original/start/current的临时预览。此项不消除State在更新预览时保存必要几何状态的副本，不宣称全部几何缓存成本已消失。
- UI196通过/1忽略（target/annotation-software-document-ui-fixed.log）。扩展既有20000点/普通文字/序号说明/替换与追加/100%与125%DPI回归：直接软件绘制与拥有预览像素完全相同，缓存软件绘制与拥有SceneCompiler结果完全相同；原生对照仍最大通道误差≤1；后台Snapshot对象与拥有预览相同，重复编辑视图复用Arc（target/annotation-software-document-regression-final.log通过）。首轮编译修正迭代方法遗漏及显式借用生命周期；未放宽断言。一次编辑脚本未匹配格式化后的文本、在写入前失败，随后重跑正确脚本和最终回归，没有把先前缓存测试作为新断言证据。
- 本批未改变GPU调度或测量实际显示/输入延迟，未保存真实App/DPI/IME画面。剩余菜单/窗口所有权、预览状态成本、故障恢复、GPU呈现门槛及完整应用验收继续保留。workspace447通过/39忽略，包含Slint编译（target/annotation-software-document-workspace.log）；workspace/all-targets Clippy -D warnings通过（target/annotation-software-document-clippy.log）；Windows workspace Release通过（target/annotation-software-document-release.log）；git diff --check通过。未暂存、提交或推送。


## 撤回无显示间隔收益的调度实验，菜单拥有返回任务（2026-10-03）

- 提前效果准备实验将交换链许可检查移到效果更新与输出比较之后，后缓冲区写入仍受许可保护，阻塞时仅实际效果更新后 Flush。原生单次源变化回归额外检查等待期间已准备绿色效果纹理，通过。35 秒 Release 高强度模糊夹具测得系统显示版本 59.403FPS、QPC 间隔 P95 27.784ms、GPU P95 2.837ms、取帧到首次提交 P95 8.792ms。显示间隔未改善，因此只撤回本实验的门控移动、额外 Flush 与实验专属准备断言，保留原单次源变化、等待、拒绝提交及后续成功像素回归。撤回后该原生测试通过（target/annotation-effects-before-swap-reverted-regression.log）。实验结果见 target/annotation-effects-before-swap-large-blur.log，不属于最终代码的性能证明。
- 选色器返回设置面板与水印时间菜单结束的 16ms 任务由菜单 State 持有 transition Timer，不再使用全局 single_shot/next_frame。take 在替换/关闭时同时停止准备、定位和返回任务；State 销毁自动销毁 Timer。返回逻辑仍校验会话、面板/水印编辑实例、菜单及参数目标，不改变 Escape 的单层关闭保护或参数/焦点规则。
- 扩展菜单所有权测试使用受控时钟，验证正常返回执行、take 后不运行返回，以及直接丢弃 owner 后不运行返回，保留窗口弱引用及原生监控释放断言。首轮局部测试筛选用了源文件名而非实际模块别名，运行 0 测试，不作为通过证据；后续完整 workspace 实际执行 annotation::choice::tests::menu_owner_cancels_work_and_releases_window_monitor_on_drop 并通过。
- workspace 447 通过/39 忽略（target/annotation-menu-transition-workspace.log），包含 Slint 编译；workspace/all-targets Clippy -D warnings 通过（target/annotation-menu-transition-clippy.log）；Windows workspace Release 通过，2m16s（target/annotation-menu-transition-release.log）；git diff --check 通过。未新增 Core 接口、配置、crate 或依赖，未暂存、提交或推送。
- 本批只补齐两个菜单延迟任务的所有权，并未完成菜单整体 Registry 解耦或所有编辑事件迁移；未保存真实 App/DPI/IME 画面。GPU 间隔门槛、源像素到实际显示、输入延迟、设备恢复及完整生命周期验收仍未完成，目标保持进行中。


## 文字编辑实例拥有输入更新与完成任务（2026-10-03）

- 普通文字与序号说明的内容变化/光标变化不再各自调用全局 later 来重复排队布局。TextEdit 拥有 Rc<InputWork>，同一轮事件更新使用可替换的零延迟 Timer；执行时仍调用 resize_text_editor 读取最新内容、组字与选区。完成、取消及原生关闭请求使用另一实例 Timer，首个完成请求优先，停止尚未执行的布局更新；完成路径按最新实际值生成文本及边界，不依赖旧布局结果。
- 通过 TextEdit::bind_input 接入真实 Slint 回调，无需在控件回调内借用 Registry。任务执行入口保留会话/编辑身份校验。State::take/clear 的 retire 停止并失效输入工作，即使外部仍持有组件，旧回调也无法再安排任务；Timer 不捕获窗口强引用或自身 Rc，避免循环所有权。准备和光标时钟沿用既有实例所有权。
- 新受控时钟回归通过真实 content/caret/done/cancel 回调验证：多次中文输入与光标通知合并为一次更新，读取最新值与 UTF-8 选区；take 与 owner 销毁后保留的旧窗口不能执行或重排任务；取消优先于随后重复完成，并停止待布局。现有目标身份回归改为在 Registry 被借用时触发真实输入与完成回调，定时执行后普通文字/序号说明在 100%/125% DPI 仍提交到索引移动后的原对象，缺失对象不复活。局部回归日志 target/annotation-editor-input-regression.log、annotation-editor-input-target.log；扩展后的所有断言纳入最终 workspace。
- 最终 workspace 448 通过/39 忽略，UI 197 通过/1 忽略，包含 Slint 编译（target/annotation-editor-input-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-editor-input-clippy.log）；Windows workspace Release 通过，2m16s（target/annotation-editor-input-release.log）；git diff --check 通过。未新增 Core 接口、配置、crate 或依赖，未暂存、提交或推送。
- 本批没有修改 GPU 路径或重测性能，没有真实 App/DPI/IME 交互截图。仍需完成菜单协调、其他窗口回调所有权、手势收尾、增量失效与失败恢复，以及显示间隔/输入延迟和资源释放验收。组件与受控时钟测试不替代实机中文输入法验证。


## 活动手势取消共用收尾，修复切换箭头后的旧绘制提交（2026-10-03）

- 审计发现 set_mode 原来只首先取消对象变换/序号放置，随后按目标工具分别清理草稿。切换到 Tool(2) 时不会进入非箭头分支，矩形/铅笔的旧 Draw 和草稿可能保留，旧捕获的松键会提交旧工具对象。现所有实际模式切换先 cancel_active_gesture；重复选择相同模式仍沿用原规则。
- cancel_active_gesture 共用 Core cancel_drag、活动编辑文字几何恢复、编辑选择锚点释放、gesture/gesture_object/draft/arrow_pending/polyline/last_polyline_click/pending_edit_panel 清理、参数回填和序号计数同步。没有活动按键手势但仍在等待第二点的箭头/折线也可退休。cancel_object_gesture 与 cancel_sequence_placement 保留原触发条件，委托相同收尾，避免改变撤销/删除的命令范围。画布捕获取消和文字框角点取消接入同一规则；Escape 保留菜单/编辑窗口优先层级。
- 马赛克首次采集验证依然由 mosaic coordinator 拥有，不在通用手势函数里无差别删除 pending 对象。捕获取消先回滚手势再取消 pending；Escape 的 pending 分支也先清理活动操作，避免仅清 gesture 而保留 geometry snapshot。
- 新真实 canvas_pointer 组件回归覆盖矩形/铅笔、100%/125% DPI、负坐标画布：按下移动产生草稿，再切换到箭头、Escape 或捕获取消，随后到达旧松键；不能产生对象或撤销记录（target/annotation-gesture-retirement-regression.log）。既有移动/缩放命令回归追加 Escape，验证原对象恢复且历史仍只保留原创建记录。现有序号取消、文字尺寸、面板保持展开与马赛克取消测试一同通过。
- 最终 workspace 449 通过/39 忽略，UI 198 通过/1 忽略，含 Slint 编译（target/annotation-gesture-retirement-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-gesture-retirement-clippy.log）；Windows workspace Release 通过，2m14s（target/annotation-gesture-retirement-release.log）；git diff --check 通过。未新增 Core 接口、配置、crate 或依赖，未暂存、提交或推送。
- 本批未修改 GPU 或重测性能，未保存真实 App/DPI/IME 画面。手势生命周期已减少重复取消逻辑，但提交路径、连续参数事务与完整真实交互仍需继续审计；菜单协调、故障恢复、源像素到实际显示、性能门槛和资源释放验收尚未完成。


## 独立核验真实 WGC 源像素与冻结提交候选（2026-10-03）

- 新增仅 cfg(test) 的 ignored 原生夹具 gpu_owned_source_pixels_match_frozen_commit_versions；不修改生产采集、绘制、调度或性能计时。持有无激活源窗口，四个已知 GDI 色带画面各执行高强度模糊及中强度马赛克。真实 Backend::sample_gpu 返回的拥有窗口纹理经测试用阻塞 readback 读取，每个色带在三处高度共 24 个内部采样点识别已绘制版本，独立建立 (token,sequence,ticks) 到像素版本的对应。同一版本再次读到不同画面会失败。
- Renderer 使用该实际 WGC 样本，实际 native commit 前读回后缓冲区相同采样点，成功提交后用 last_commit_sources 对照其冻结来源。对两个效果分别拒绝一个 37 候选，源窗口继续绘制 109；拒绝重试直到确认实际 WGC 纹理已读到 109，再允许旧候选提交。旧候选输出必须仍与其冻结的 37 来源一致，后续 109/211 也必须成功提交。最终两次拒绝候选、四个画面版本与两个效果均验证通过，最大采样点通道误差 0（target/annotation-owned-pixels-native-final.log）。
- 初次编译发现 render 可变借用与验证 graphics 借用重叠，测试验证对象改为共享同一 device/context 的独立 Graphics。首轮执行读到窗口默认 240 灰色，诊断具体像素后确认 STATIC 初始 WM_PAINT 覆盖第一次 GDI 发布；先处理初始绘制消息后再发布测试色带，严格原误差门槛通过。没有跳过错误样本或放宽精度。Clippy 指出常量 assert，改为已有夹具同款 Release 检查，无 lint 豁免。
- 最终 workspace 449 通过/40 忽略，新增原生夹具另行显式执行通过；含 UI 198 通过/1 忽略及 Slint 编译（target/annotation-owned-pixels-workspace-final.log）。workspace/all-targets Clippy -D warnings 通过（target/annotation-owned-pixels-clippy-final.log）；Windows workspace Release 通过（target/annotation-owned-pixels-release.log）；git diff --check 通过。Release 为缓存构建，测试代码不进入产品二进制。未新增 Core 接口、配置、crate 或依赖，未暂存、提交或推送。
- 证据范围严格为拥有源 WGC 纹理与实际提交候选的指定色带采样点；不证明整张图每个像素、任意动态视频、光子显示、输入到实际显示延迟或实时性能收益。阻塞读回仅用于正确性验证，不可拿本夹具计算生产延迟。原有呈现 QPC、真实 App/DPI/IME、故障恢复及资源释放验收继续保留，完整目标未完成。


## GPU 重建保留最后成功呈现的窗口（2026-10-03）

- 原恢复分支立即丢弃 Renderer，连同成功显示的 Surface 一起销毁。现 worker 将已呈现 Surface 移入独立 retained_surfaces，再销毁失败采集、效果、光栅和旧 Renderer；未成功呈现的后缓冲区不能替换保留显示。替代后端实际成功呈现后释放保留窗口；clear 的 generation 更换、不同 session 及 worker 退出均释放。资源销毁在 mailbox 锁外，保持 clear/输入可取消。
- 保留表面不能依赖已退休 Backend 的 COM apartment。复用平台内部 Apartment RAII guard，使 worker 在首次设备准备时持有独立 apartment，生命周期覆盖当前及保留显示；失败进入既有一次重建预算，不提前退出并留下可接受 submit 的空 worker。Backend 构造也使用同一 guard，构造失败自动平衡初始化，代替手工 RoUninitialize 分支；无额外设备或源生产者，不新增 Core 接口。
- 扩展真实桌面原生回归注入设备损失，并通过仅测试的 defer_device_rebuild 暂停替代创建。恢复期间连续 20 次桌面 GetPixel 验证上一成功色值，原生呈现窗口始终只有一个；恢复成功显示新颜色且旧窗口释放。另起拥有端口，在保留显示且尚无替代设备时 clear，窗口数归零、latest 清空、join 后无残留。原有一次恢复预算、重复损失、跨 session 缓存和取消提交断言继续通过（target/annotation-recovery-display-native-final.log）。此为故障注入和桌面像素采样，不宣称真实驱动崩溃在所有硬件上的表现。
- 实际 WGC 源/冻结候选像素回归显式通过，最大采样误差仍为 0（target/annotation-recovery-display-owned-pixels-final.log）。最终 workspace 449 通过/40 忽略，含 UI/Slint（target/annotation-recovery-display-workspace-final.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-recovery-display-clippy-final.log）；Windows workspace Release 通过，1m10s（target/annotation-recovery-display-release-final.log）；git diff --check 通过。未新增配置、crate、依赖或 Core 公开接口，未暂存、提交或推送。
- 本批只修复有界重建等待期间的旧显示所有权。UI Presenter.fail 仍会 clear 原生端口，终止恢复到软件首帧的交接还需继续实现及验证，不能据此称整个降级无空档。未重新测量正常链路性能或真实 App/DPI/IME，物理显示延迟、设备失败实机、资源与完整重构验收尚未完成。


## GPU 失败到软件首帧的交接（2026-10-03）

- Presenter.fail 对已经激活的原生显示保留端口所有权，停止原生轮询并禁用本会话 GPU 重试；软件仍可绘制。只有当前完整效果及所有画布的软件提交成功后，finish_software_handoff 才 clear。初次未显示的失败仍立即清理；重复失败、重复交接和会话关闭保持幂等。Presenter 自有单次定时器重试软件提交，关闭时停止。
- 软件 Frame 携带其不可变文档快照及完整性标记。交接要求完整输出、当前 context 与当前快照身份一致；临时草稿及部分采集输出不能授权交接。任务去重与结果接纳也比较快照身份，防止可见 key/context/capture serial 相同但完整文档变化被误去重。Snapshot 准备现在发生在去重之前，可能增加重复 UI 更新的完整比较成本，尚未测量，不声称性能收益。
- 两画布组件回归验证第二块画布提交失败时保留原生画面，重试成功后一次清除，两个软件结果均与原基线相同。夹具最初依赖窗口位置辨认第二窗口，但 MinimalSoftwareWindow 不保留原生位置；修正为 Slint 窗口实例身份，没有放宽断言。State 回归覆盖不完整输出、context 改变、同可见 key 的元数据改变与快照退休；worker 原有分块/部分采集回归补充真实输出 complete 标记及零强度无采集完整帧断言。
- 当前 workspace 451 通过/40 忽略，UI 200 通过/1 忽略，含 Slint 编译（target/annotation-software-handoff-workspace.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-software-handoff-clippy.log）；Windows workspace Release 通过，2m17s（target/annotation-software-handoff-release.log）。随后仅增加 worker 测试断言，15 项相关回归通过（target/annotation-software-handoff-worker-final.log）。git diff --check 通过。
- 本批证据为组件模拟端口与软件 worker 回归，没有实际应用 GPU→软件交接截图、物理显示连续性或设备驱动失败验证。已提交软件画布可能在整组就绪前覆盖部分旧显示，未证明跨屏原子呈现；永久软件采集不可用时仍保留最后成功原生画面，关闭会释放。真实 DPI/IME、资源测量、正常链路性能与完整重构验收继续保留。未新增 Core 接口、依赖、配置或 crate，未暂存、提交或推送。


## 会话拥有画布渲染与放大镜首次初始化任务（2026-10-03）

- 将 schedule_render 和首个放大镜初始化的全局 single_shot 迁入 annotation_frame::Tasks，Registry 持有其两个 Timer。渲染请求按实际 timer.running 合并，不再另存可能与真实任务脱节的 render_queued 布尔值；直接 render_canvases 时停止尚未执行的排队渲染。保留 16ms 时序与原 generation/revision 校验。
- 放大镜延迟任务只携带会话与修订身份，不拥有窗口；新请求替换旧请求，放大镜 Clear 取消尚未执行的初始化。会话 close 清理两个任务，Tasks Drop 同样停止定时器。没有改变输入焦点、放大镜效果、GPU 调度或其他工具的创建规则。
- 新受控时钟回归验证重复渲染只调用一次、放大镜只执行最新修订、clear/Drop 后没有回调，以及新 owner 可重新排队。首轮测试错误假设同 deadline 的两个独立 Timer 保证回调顺序；实际顺序相反，修正为比较两项完整结果且不要求顺序，没有放宽次数或身份断言。已有 retired_toolbar 回归补充真正 Registry.close 前后两个 timer 的 pending 状态，随后旧工具条操作仍不能影响重开的会话。
- 当前 workspace 452 通过/40 忽略，UI 201 通过/1 忽略，含 Slint 编译（target/annotation-frame-tasks-workspace-final.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-frame-tasks-clippy-final.log）；git diff --check 通过。Windows workspace Release 通过（target/annotation-frame-tasks-release-final.log）。
- 本批没有测量帧率、分配量或真实 App/DPI/IME，不声称性能指标改善。剩余 ZERO 事件转发及窗口初始化任务仍需按拥有边界审计；菜单对 Registry 的协调依赖、完整手势提交、热路径及真实呈现验收尚未完成。未新增 Core 公开接口、crate、配置或依赖，未暂存、提交或推送。


## 菜单拥有输入队列与连续参数收尾（2026-10-03）

- 普通菜单、描边/背景面板、选色器和水印时间菜单的 Slint 输入回调及初次显示、原生 dismissal 转入本菜单实例的 InputWork。队列按 FIFO 执行，同轮任务先取出批次再执行，回调不需要在事件接收时借用 Registry；任务执行仍保留原会话、面板、菜单、目标身份校验。没有将原本有顺序要求的参数更新改成只保留最后一项。
- Menu 退休与 Drop 都停止输入 Timer 并清空 pending；正在执行批次每项前检查 active，关闭时剩余项失效。外部保留的旧 Slint 组件即使触发回调，也不能重新排队。Timer 弱引用 InputWork，输入任务只捕获值/身份，避免窗口和队列循环所有权。菜单返回、准备及定位仍由 State 原有 Timer 拥有。
- 连续改色松键的两轮 ZERO 收尾改为同实例 after_inputs；先处理已排队值，再结束 Core 参数事务。菜单替换时取消收尾，显式 close 仍完成已应用的参数事务并恢复焦点。保留二级工具条关闭策略和所有颜色目标的独立字段写回。
- 新受控时钟回归验证 FIFO、两轮收尾、执行中退休丢弃剩余批次、退休后拒绝重排。通过实际 Slint hex/color_dragging/cancel 回调，在 Registry 被借用时排队两次颜色更新；颜色按顺序更新，关闭前未执行值被丢弃，旧窗口不能修改参数或关闭新菜单。选中矩形连续改色，松键后三轮事件循环结束后一次撤销恢复完整原对象，再一次撤销移除创建，证明连续更新只产生一次参数记录。11 项菜单回归及独立撤销回归通过。
- 当前 workspace 453 通过/40 忽略，UI 202 通过/1 忽略，含 Slint 编译（target/annotation-menu-input-workspace-final.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-menu-input-clippy-final.log）；git diff --check 通过。Windows workspace Release 通过（target/annotation-menu-input-release-final.log）。
- 本批没有修改 GPU 调度、字体排版或测量性能，没有真实 App/DPI/IME 画面。菜单执行仍通过 Registry 协调，所属窗口及参数应用边界尚需进一步迁移；工具条/水印入口的 ZERO 转发、完整手势提交、性能及真实呈现验收继续保留。未新增 Core 接口、依赖、配置或 crate，未暂存、提交或推送。


## 菜单完成决策与会话应用分离（2026-10-03）

- 将 finish 中普通菜单、选色器返回与水印预设的判断提取为 completion_plan。输入仅为菜单展示请求、父面板请求、所属水印实例、工具及选择索引；输出明确的 SelectionEffect 和 CloseAction，不访问 Registry 或文档。参数选择仍由会话 apply_menu_edit 校验目标并更新，水印解析/模板规则不变。
- 三种返回目标以 ReturnDestination 表达：二级面板、上一级设置面板、水印输入窗口。延迟返回统一通过 ReturnToken 校验会话/菜单实例、return_pending、当前参数目标与所属窗口，再执行关闭或返回。打开父面板放在 Registry 借用结束后，避免重复嵌套读取全局状态。保留下一帧 guard，重复 native/Slint Escape 仍只关闭一层。
- 新纯函数回归检查有效字体索引与独立参数事件、取消/非法索引、颜色关闭延迟、父设置面板优先级、水印合法预设及非法索引、既有保持菜单的选择规则。原 11 项菜单回归及真实输入 FIFO/连续改色撤销继续通过；完整 UI 同时覆盖水印窗口和时间菜单关闭重开、旧窗口事件及重复 Escape。
- 当前 workspace 454 通过/40 忽略，UI 203 通过/1 忽略，含 Slint 编译（target/annotation-menu-completion-workspace-final.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-menu-completion-clippy-final.log）；git diff --check 通过。Windows workspace Release 通过，2m17s（target/annotation-menu-completion-release-final.log）。
- 本批改善菜单职责和返回校验的可推理性，没有减少所有 Registry 协调调用，也没有重测 GPU 性能或保存真实 App/DPI/IME 画面。剩余工具条/水印输入任务、窗口所属关系、完整手势提交、热路径与实机验收继续保留。未新增 Core 公开接口、依赖、配置或 crate，未暂存、提交或推送。


## 工具条与水印窗口共用实例输入所有权（2026-10-03）

- 将菜单 FIFO InputWork 移到 annotation_window，复用明确的临时窗口输入边界。Slot 持有当前实例队列，open 创建新队列，clear/Drop 首先退休旧队列，再停止准备/定位并释放组件与原生接线。没有长期保留隐藏窗口，也没有把正文编辑的最新值合并语义改为 FIFO。
- 主工具条命令、二级工具条参数/选择/快捷键及水印输入的时间菜单/应用/取消接线捕获打开时的 InputWork；事件接收不重新查找新实例队列。二级工具条原生 dismissal 与松键后的焦点恢复也使用绑定时的队列。仍保留每个执行入口的 generation/revision、参数目标与投影身份校验；参数目标继续在事件发出时捕获，避免后续选择变化改变事件含义。
- 新 Slot 受控时钟回归通过实际水印组件 apply 回调：活动输入正常执行，关闭前 pending 输入丢弃，保留旧组件仍不能排队；释放旧组件及队列后 Weak 均不能升级，证明无回调/队列循环。直接 owner Drop 同样取消任务。主工具条关闭重开回归追加真实队列 pending/retired 断言，旧快捷键不能重排；已有参数来源、焦点、二级保持展开、连续参数撤销及水印旧窗口回归继续通过。
- 当前 workspace 455 通过/40 忽略，UI 204 通过/1 忽略（target/annotation-window-input-workspace-final.log），git diff --check 通过；workspace/all-targets Clippy -D warnings 通过（target/annotation-window-input-clippy-final.log），Windows workspace Release 通过（target/annotation-window-input-release-final.log）。首轮 UI 203 通过/1 忽略，随后添加窗口拥有回归纳入最终 workspace。此批没有修改 Slint 源文件，UI 构建覆盖既有 Slint 编译产物接线。
- 本批没有重测 GPU/显示性能或保存真实 App/DPI/IME 画面。画布输入和剩余会话级 ZERO 转发仍须审计，菜单实际执行仍需要 Registry 协调。完整手势提交、热路径成本、跨屏资源与真实呈现验收保留。未新增 Core 接口、crate、配置或依赖，未暂存、提交或推送。


## 画布输入退休与剩余全局延迟转发迁移（2026-10-03）

- 每屏 CanvasLayer 持有复用的 window::InputWork，键盘/完成路径/关闭命令在所属画布排队，Layer Drop 在隐藏窗口前退休队列。原生移动、按下、松开、捕获取消和滚轮仍按原同步路径处理，不改为异步采样或改变手势顺序。保留 generation、索引及画布 revision 校验。
- 审计发现 canvas_pointer 在选中对象后使用无身份 later 闭包自动打开二级面板，执行时读取任意当前 Registry 与模式。现 queue_geometry_panel 捕获来源会话、画布实例、对象 ID 和工具组，只在当前选择仍匹配且不是鼠标穿透时打开。关闭/替换画布会取消排队，身份校验仍防守直接过期调用。
- 静止指针光标复查改为画布拥有任务，并新增 revision 校验，避免同会话同索引的替代画布接收旧位置复查。参数应用、正文选择及文字角点更新直接调用 Registry.queue_render，由 frame::Tasks 持有 16ms Timer，不再经过一轮全局 ZERO 转发。移除最后的 later helper；当前 annotation*.rs 无 Timer::single_shot 或 later 调用。未声称已消除所有 Registry 协调或测得性能改善。
- 新受控时钟回归覆盖选中状态改变、鼠标穿透、同索引画布替换及关闭重开，旧自动面板请求均无效果；当前请求正常打开。已有保留旧画布回归新增实际 escape 回调在 Registry 借用期间排队、替换时退休和旧控件不能重排断言。Layer 生命周期回归验证 Drop 取消 pending 输入及定位任务。
- 当前 workspace 456 通过/40 忽略，UI 205 通过/1 忽略，含 Slint 接线构建（target/annotation-canvas-input-workspace-final.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-canvas-input-clippy-final.log）；git diff --check 通过。Windows workspace Release 通过（target/annotation-canvas-input-release-final.log）。
- 本批未修改 GPU 运算/采集调度，没有真实 App/DPI/IME 画面或输入到显示测量。菜单 owner 协调、完整手势提交/参数事务、失效热路径与真实故障/呈现验收尚未完成。未新增 Core 公开接口、依赖、配置或 crate，未暂存、提交或推送。


## 手势收尾只操作自己拥有的文档事务（2026-10-03）

- 审计发现 canvas_pointer 的松键兜底在没有活动 Gesture 时也调用 Session.finish_drag；连续参数操作仍标记 active，但 Core 快照已被取走，后续更新会另压历史。现在无手势的 release 不提交；通用变换收尾只对 owns_document_transaction 的手势 finish。
- Gesture 明确哪些状态拥有 Core 快照：序号放置、移动、缩放、非编辑模式且已越过阈值的文字控制点拖动。纯 Draw、折线/箭头临时状态、正文选区及编辑实例内部几何没有 Core 快照所有权。cancel_active_gesture 只对拥有者 cancel_drag，仍清理草稿、选择锚点、编号投影及光标，不回滚连续参数的独立快照。
- 新 UI 回归在 100%/125% DPI 和负坐标画布下执行实际参数入口：第一次改色、无手势松键、第二次改色、完成参数事务，一次撤销必须恢复整对象，再次撤销移除创建；取消纯聚光草稿后全局不透明度仍保持新值，完成后一次撤销恢复原不透明度。首轮夹具 defaults 参数类型错误已修复；重复用例继承前一轮 UI 参数导致无变化更新，改为每例独立 State，不放宽断言。
- 当前 workspace 457 通过/40 忽略，UI 206 通过/1 忽略，含 Slint 接线构建（target/annotation-gesture-transaction-workspace-final.log）；workspace/all-targets Clippy -D warnings 通过（target/annotation-gesture-transaction-clippy-final.log）；git diff --check 通过。Windows workspace Release 通过，2m58s（target/annotation-gesture-transaction-release-final.log）。
- 本批未修改 Core 公开接口、GPU、采集或 UI 样式，未测量性能或保存真实 App/DPI/IME 画面。活动文档拖动期间到达的延迟参数回调仍需继续审计其序列化，不能把当前所有权保护当作全部手势/参数边界已完成。菜单所属关系、热路径及真实故障/呈现验收继续保留。未新增 crate、配置或依赖，未暂存、提交或推送。


## 活动文档手势与延迟参数分离（2026-10-03）

- 活动移动、缩放、序号放置及已激活的非编辑文字角点手势期间，合法参数更新先进入 UI 等待队列，不调用 parameter_edit.touch，不共享或提前结束手势 Core 快照。同一目标保留最新完整值；菜单字段更新从待应用值继续修改，面板显示该值，避免后续选择抹掉前次待应用字段。
- 手势完成后、重新投影参数前应用等待值；取消先恢复 Core 手势快照，再应用仍具有原目标身份的参数。序号完成先结束放置事务、应用原默认目标，再选中新对象，避免把默认样式回写到刚创建对象。面板退休清除队列；执行仍校验面板、对象、编辑和投影身份。没有添加轮询定时器或逐帧重试。
- 新受控时钟回归覆盖选中矩形移动、600ms 等待、松开与取消、100%/125% DPI 和负坐标：拖动期间对象颜色不变且没有参数事务；结束后最新颜色应用；撤销参数、移动和创建分别恢复。第一轮夹具未执行选中对象正式参数回填，鼠标移动导致 projection 更新使请求过期；补齐 restore_parameter_projection 后断言通过，没有削弱身份检查。最终 workspace 另补菜单字段选择保留待应用颜色的断言。
- 针对性回归通过（target/annotation-deferred-parameters-regression-final.log）。当前最终 workspace 458 通过/40 忽略（UI 207 通过/1 忽略，含 Slint 接线构建）；workspace/all-targets Clippy -D warnings 通过（1m11s），Windows Release 通过（29m38s，target/annotation-deferred-parameters-release-final.log），仅对应当时批次，不替代随后文字控制点及取消修改的门禁。日志为 target/annotation-deferred-parameters-{workspace,clippy,release}-final.log；尚不宣称全部门禁通过。
- 本批没有修改 GPU 调度、采集、Core 接口、配置、依赖或 UI 样式，没有实机 DPI/IME 或性能测量。新等待路径的序号/文字角点需要继续专门验证，马赛克待采集取消与文档快照边界仍需审计。完整目标继续保持进行中；不暂存、提交或推送。


## 文字控制点激活与参数等待交接（2026-10-03）

- update_text_handle 首次越过阈值时读取当前文档对象或编辑草稿，再结束已有参数事务；非编辑模式随后开始独立 Core 快照。修复按下时 gesture_object 覆盖阈值前已应用颜色/字体/描边，以及 begin_drag 复用参数快照的问题。没有改变 3×DPI 阈值、抓取偏移、最小字号或旋转锚点。
- Gesture.blocks_parameter_edits 区别于 owns_document_transaction：编辑中的 active 文字控制点虽不拥有 Core 几何事务，也会反复写回对象副本，因而同样等待参数；完成时先退休手势、应用等待值，再投影缩放后的字号。取消复用原路径，先恢复几何再应用仍有效的等待参数。无新增轮询、Core 公开接口、配置、依赖或 crate。
- 新文字对象回归覆盖 100%/125% DPI、负坐标、阈值内移动、阈值前改色、激活后再次改色，以及完成和取消：后续参数、缩放、前次参数和创建具有正确的独立撤销顺序。针对性测试通过（target/annotation-text-handle-transaction-regression.log）。草稿回归验证正文不丢失、取消恢复几何但保留参数、无文档对象/撤销，以及纯分类回归，均在最终 workspace 单元测试通过。当前最终 workspace（含 doc-tests）461 通过/40 忽略（UI 210 通过/1 忽略，含 Slint 接线构建）；workspace/all-targets Clippy -D warnings 通过，1m24s。Windows Release 通过（14m56s，target/annotation-text-handle-transaction-release-final.log）；后续 pending 取消与平台夹具修改需各自门禁覆盖。
- 新源码完整门禁已启动，串行 session 1631，日志 target/annotation-text-handle-transaction-{workspace,clippy,release}-final.log。上一批 Release session 20628 仍在构建且未重启；其结果只记为上一批边界，不替代当前源码门禁。git diff --check 通过。
- 本批尚无真实 App/DPI/IME 截图、性能或输入到显示证据；序号默认目标、面板退休队列、马赛克 pending 重复取消、热路径及完整 GPU 性能/故障验收仍未完成。不暂存、提交或推送，完整目标继续进行。


## 待验证马赛克取消与参数事务边界（2026-10-03）

- Escape 的 pending 分支移除无条件 Session.cancel_drag：原路径即使没有文档手势也可能回滚参数快照，令仍显示的新不透明度丢失。活动手势由统一 cancel_active_gesture 拥有，待验证遮罩由 mosaic::cancel_pending 清理。
- 专用协调器取消待验证遮罩的移动/缩放改用同一手势入口，清理备份和重放等待参数。将 finish_style_edit 放在手势取消后、discard_mosaics 前，使接受的无关参数先独立完成，再从当前文档及历史中移除失败遮罩；没有更改 Core 丢弃历史规则。
- 新回归覆盖负坐标和创建 DPI 1/1.25：无手势参数调整后 Escape、移动待验证遮罩时协调器直接取消，以及移动时 Escape。要求不透明度保留、队列与手势/事务清空、一次撤销恢复无关参数、另一次撤销移除原矩形；失败遮罩和其移动不留历史。针对性测试已通过（target/annotation-pending-cancel-regression.log）；最终 workspace 462 通过/40 忽略（UI 211/1，含 Slint 接线构建与 doc-tests）；workspace/all-targets Clippy -D warnings 通过，38.67s。Windows Release 通过，8m46s（target/annotation-pending-cancel-release-final.log）；该批完整门禁通过。git diff --check 通过。
- 上一批 Release session 20628 已 exit 0，29m38s；文字批次 session 1631 仍在 Release 编译。后续最终门禁需覆盖当前取消源码，不用先前的绿色结果替代。未新增接口、crate、依赖、配置，未暂存、提交或推送；真实应用、资源/跨屏和 GPU 性能验收继续保留。


## 显示模式只读查询（2026-10-03）

- 通过 EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS) 只读查询当前主显示模式：2560×1440、整数配置刷新率 144Hz（target/annotation-display-mode-query.log）。原生返回 dmSize 188，日志字段标为 returned_mode_size_bytes，非声称分配结构大小。没有修改刷新率、创建窗口或采集屏幕内容。
- 该结果与此前 OS counter 推算约 6.946ms 周期接近，但只覆盖查询时的主显示器整数配置，尚未绑定旧性能夹具实际 output，也不是有理数刷新率或物理显示测量。下一步将模式查询绑定拥有的源/呈现窗口，结合 present/sync refresh counter 分解采集、等待与呈现周期；不能据此宣称 20ms 门槛已通过或自行降低门槛。


## 性能夹具按拥有窗口查询显示模式（2026-10-03）

- 在 annotation_gpu/performance.rs 中接入 MonitorFromWindow/GetMonitorInfoW 与 EnumDisplaySettingsW：使用拥有源窗口和透明输入覆盖窗口分别定位最近输出，性能测量前后记录设备名、监视器物理边界、模式像素尺寸及整数配置刷新率。仅查询端点，不在热路径轮询；未知/默认刷新率和 API 错误明确 unavailable，不填零冒充有效数据。
- 报告使用 mode_endpoints_match 和 windows_start_same_output，只表达两次查询一致以及开始时两窗口属于同一模式；不声称测量期间一直稳定、实际 Surface 输出完全绑定、有理数刷新率或物理显示。保留 OS QPC/refresh counter 推算字段和所有原性能门槛；未更改算法、调度、屏幕模式或用户焦点。
- 新 ignored 原生探针以既有 WS_EX_NOACTIVATE 窗口机制创建 64×64 自有窗口，前后查询一致；实际输出 DISPLAY1、边界 [0,0,2560,1440]、模式 2560×1440、144Hz。窗口及 Backend 按拥有作用域销毁。显式测试通过（target/annotation-owned-monitor-mode-native-final.log），并不等同整段 30s 性能运行或实机 App/DPI/IME 验收。
- 首次编译受 super::* 导入的 Core Result 别名影响，以及 windows 0.62.2 Error 方法名变化；改为 std::result::Result 和 Error::from_thread，修复后原生探针通过。没有新增 Windows feature、Core 接口、依赖、crate 或配置。最终 workspace 462 通过/41 忽略（UI 211/1，Platform 95/38，含 Slint 接线及 doc-tests）、workspace/all-targets Clippy -D warnings 和 Windows Release 均通过（target/annotation-monitor-mode-*-final.log）。pending 取消 Release session 73450 也已 exit 0。git diff --check 此前通过，最终再核对。
- 尚未在安静构建环境重新运行完整性能夹具，既有 20ms 间隔与 50ms 输入到显示要求仍未证明。后续应结合此输出模式与实际呈现计数分解多余等待；再继续结构、真实应用与资源验收。不暂存、提交或推送。


## 模式查询 DPI 边界与高强度模糊重测（2026-10-03）

- 第一轮完整夹具的模式像素始终 2560×1440/144Hz，但 GetMonitorInfo 边界从 2560×1440 变为 2048×1152，端点不匹配（target/annotation-monitor-mode-large-blur-native.log）。检查 capture::Backend Drop 会恢复线程 DPI 上下文，确认查询两端处于不同 DPI 上下文；该 metadata 不能当成真实显示模式变化或一致的物理边界证据。
- monitor_mode 使用 PhysicalModeQuery 临时进入 PER_MONITOR_AWARE_V2，所有返回路径由 Drop 恢复原上下文；没有改变进程或用户显示设置，也不在渲染热路径查询。原生探针在 Backend Drop 前后查询，断言两次调用均恢复各自原始上下文、物理边界一致，并显式 IsWindow 验证拥有窗口销毁（target/annotation-owned-monitor-mode-dpi-cleanup-native-final.log）。
- 安静构建环境的完整 35s Release 高强度模糊重测通过（target/annotation-monitor-mode-dpi-large-blur-native.log）：源 60.000FPS；源/输入两端均 DISPLAY1、物理边界 [0,0,2560,1440]、模式 2560×1440、144Hz，端点匹配。OS 报告有效更新 59.700FPS、GPU P95 2.988ms、dequeue→commit P95 8.950ms、匹配 OS QPC 间隔 P95 27.784ms；周期推算中位数 6.946ms，histogram {1:407,2:606,3:447,4:300,5:28,6:1,8:1}。不是 photon 或真实 App 输入到显示测量，不声称 20ms/50ms 门槛通过。
- 本轮只是修正 measurement metadata，没有改算法/调度或证明延迟改善。144Hz 与 60Hz 变化源存在多刷新周期量化，四周期及更长间隔仍需分解；保留原目标，不自行放宽验收。实时三组场景、输入到显示、UI 等待目标边界及实机/资源验证继续保留。
- 最终源码 workspace 462 通过/41 忽略、workspace/all-targets Clippy -D warnings、Windows Release 及 git diff --check 均通过（target/annotation-monitor-mode-dpi-*-final.log）。显式原生探针和完整高模糊夹具另列，不混入默认通过计数。未新增接口、依赖、配置或 crate；不暂存、提交或推送。


## 三组场景的显示模式与性能基线（2026-10-03）

修正 DPI 查询后，三组 35s Release 夹具显式通过；源窗口/输入窗口前后均为同一 DISPLAY1、物理 2560×1440、整数 144Hz，端点匹配。没有同时运行构建任务；默认门禁结果不包含显式 ignored 夹具。

| 场景 | 源 FPS | OS有效更新 FPS | GPU P95 ms | dequeue→commit P95 ms | OS QPC间隔 P95 ms |
|---|---:|---:|---:|---:|---:|
| 大区域高模糊 | 60.000 | 59.700 | 2.988 | 8.950 | 27.784 |
| 长笔刷 | 60.002 | 59.702 | 3.178 | 9.076 | 27.784 |
| 四层重叠 | 60.001 | 58.168 | 4.145 | 9.893 | 27.784 |

证据分别为 target/annotation-monitor-mode-dpi-{large-blur,long-brush,four-overlaps}-native.log。有效更新与 GPU 门槛达成不代表整条目标完成；20ms 间隔仍失败，input_to_visible_ms 明确 not_measured。采样为 OS匹配计数/QPC，非 photon；不是实际 App 交互截图或跨屏/IME 验收。

大模糊的 owned source（ordinal 3，fixture_owned=true）WGC timestamp 间隔 P95 20.838ms，arrival 21.789ms、dequeue 21.860ms、comparison observed 3.864ms；不要误用其他窗口的 ordinal 0/1 数据。144Hz 的多周期量化已在进入效果之前存在，当前 output P95 仍约四周期，不能把全部延迟归于 shader。下一步增加源发布/冻结候选/系统呈现关联的延迟证据，定位多余周期，并讨论指标与采集/刷新周期的关系；仍保留原门槛，不通过删样本或重复版本凑有效更新。

完整目标还包含 UI 等待参数目标边界、热路径/缓存、源像素正确性、真实 App/DPI/IME/资源与 input→display；不能用这三组吞吐夹具替代。未修改算法或调度、不新增接口/配置/依赖/crate、不暂存提交推送。

## 冻结源版本到系统同步的延迟边界（2026-10-03）

- 测试诊断新增带 Instant 调用前后边界的 QPC 采样，将 DXGI SyncQPCTime 映射为 CPU 时钟区间；仅接受 PresentRefreshCount 与 SyncRefreshCount 相同的样本。按确切源 token、sequence、ticks 与 native chain/present count 关联取帧、提交完成及系统同步，拒绝重复计数的歧义来源。没有按最近时间戳猜测版本。
- 延迟样本限定单 Surface，因为当前 Presentation.sources 是整帧依赖，不能证明另一个监视器的具体像素。排除混合源版本、边界外版本、时钟或顺序异常，重复版本只计第一次合法样本。无样本报告 unavailable，不填零；输入到显示仍明确 not_measured。
- 官方 [DXGI_FRAME_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics) 区分呈现刷新计数与系统采样同步计数，因此保留计数一致性检查。这里是 OS 同步统计，不是物理发光或真实 App 输入延迟；调用上下界也不涵盖显示器扫描和设备误差。
- 五项 display_timing 回归通过，包括计数跳跃、链身份、重复计数、刷新周期和时钟映射；另有精确版本关联、多 Surface 拒绝、混合与非法顺序回归通过。证据 target/annotation-sync-display-regression.log 和 target/annotation-presentation-clock-bounds-regression.log。
- 安静 Release 高强度模糊夹具显式通过，35.33s（target/annotation-sync-large-blur-native.log）：1,754 个合法延迟样本，invalid/boundary/mixed/multi_surface 均为 0，340 次重复版本排除。dequeue→sync P95 下/上界 31.142300/31.142400ms；submit completion→sync 20.151800/20.151900ms；sync→CPU observation 10.619000/10.619100ms；QPC 调用跨度 P95 0.000100ms。这些独立百分位不能直接相加或相减分解整体 P95。
- 同次有效更新 58.468FPS、GPU P95 2.880ms、dequeue→commit 9.020ms、OS QPC 更新间隔 P95 27.784ms。source/input 模式端点匹配。与旧基线不同运行，不据此宣称性能改善；本批没有改生产调度或效果算法。提交后的等待是后续实验方向，不据此确定具体成因或声称已满足 20ms/50ms。
- 当前源码 workspace 465 通过/41 忽略（Platform 98/38，UI 211/1，含 doc-tests）、Clippy workspace/all-targets -D warnings、Windows Release 及 git diff --check 均通过；串行 session48497 已 exit 0，Release 用时 1m15s。日志 target/annotation-sync-bounds-{workspace,clippy,release}-final.log。不新增接口、依赖、配置或 crate，不暂存提交推送。完整目标仍未完成。

## 序号默认参数在放置取消后的目标保留（2026-10-03）

- 新回归发现取消路径会丢弃合法参数：旧序号被选中时开始新放置，期间修改默认颜色与下一编号，取消先恢复旧选择；随后 parameter_target.object 改变，等待请求被原校验丢弃。完成路径保持默认目标正常，取消路径在 color_index 断言失败。夹具先修正字段名与 badge 必须布局后再 add 的准备错误，随后确认上述业务失败，没有降低断言。
- 将参数入口校验/等待与执行拆为 apply_geometry_values 和 apply_parameter_values。序号执行读取并写入请求携带的明确 object/default 目标，避免依据恢复后的当前选择改错对象。所有其他参数仍经原入口校验、事务合并和结果刷新。
- 取消重放仅对 PlaceSequence 的默认请求开放：tool10、无对象/编辑目标、仍处于默认绑定，且面板、投影、watermark/editor 身份一致；只允许本次取消恢复的 selection 差异。面板退休会清空等待队列，重新打开后的旧回调不能使用这个路径。没有临时修改 Core 选择或放宽通用过期事件校验。
- 回归覆盖 DPI1/1.25、负坐标、已有选中序号、新序号完成与取消：默认颜色和 next_sequence20 保留，旧对象不变，新对象维持按下样式；编号参数撤销恢复原编号与选择，再撤销放置和原创建。另验证面板退休/reopen 清除等待请求、拒绝旧目标、不产生额外历史。针对性证据 target/annotation-sequence-default-target-regression.log 与 target/annotation-sequence-panel-retirement-regression.log。
- 当前 workspace467通过/41忽略（UI213/1，含 doc-tests）、Clippy workspace/all-targets -D warnings、Windows Release、git diff --check均通过；Windows Release 串行 session96974已exit0，2m19s。日志 target/annotation-sequence-target-{workspace,clippy,release}-final.log。本批不涉及 Slint 外观、GPU 算法或调度，没有实机 IME/DPI 截图；不新增接口、依赖、配置或 crate，不暂存提交推送。字段级参数补丁、协调器职责、性能和真实应用验收继续推进。

## 面板排队输入与菜单更新按字段合并（进行中，2026-10-03）

- 实际 AnnotationPanel 回调回归复现：先排队将矩形线宽设25，随后菜单设置颜色[11,22,33]，旧完整快照执行后将颜色恢复为[217,71,43]。原路径确定失败，不是推测。
- 参数模块新增 Patch，记录通知前后值，只合并变化字段到当前接受/等待参数。完整字段解构确保新增 Slint 参数时编译提醒协议补齐。面板实例维护观察基线，Registry 持有期间的程序回填更新基线但不排队为用户操作；旧窗口身份校验与实例队列退休继续保留。
- 新回归现已通过，线宽25和菜单颜色同时保留，连续调整仍一次参数撤销（target/annotation-parameter-field-merge-regression.log）。临时门禁workspace468/41、Clippy通过；Release session26541活跃，仍不能当成最终批次完成。
- 审计发现必须补齐的生产调用方：brush_size_scrolled 在持有Registry时通过 panel.invoke_values_changed 模拟用户事件，会被回填过滤忽略。下一步改为直接调用受验证参数入口并补滚轮回归，再重新验证最终源码。本次尚无实机交互截图，不声明字段更新迁移已全部完成。

## 字段合并的滚轮调用方收尾（2026-10-03）

- brush_size_scrolled 的面板展开分支现在设置组件值后取得当前 Target，直接调用 apply_geometry_values；不再通过 invoke_values_changed 模拟用户通知。参数修改与程序回填明确分开，保留参数事务、创建 DPI 和现有尺寸限制。
- 新回归在 DPI1/1.25、负坐标画布，分别覆盖未选中后续笔刷与选中既有笔刷：连续两次滚轮由12到14，组件与参数一致；已有轨迹直径14×creation_scale，Blur/strength70不变；一次撤销恢复原对象，另一次撤销创建。默认调整不创建文档或撤销。target/annotation-field-patch-wheel-regression.log显式通过。
- 前一阶段 session26541 已exit0，Release2m18s，但源码期间新增了滚轮入口与回归，不用其结果替代最终门禁。最终源码 workspace469/41（UI215/1）、Clippy workspace/all-targets -D warnings、Windows Release及diff check均通过；串行session54975已exit0，Release2m18s。日志 target/annotation-field-patch-wheel-{workspace,clippy,release}-final.log。没有实机IME/DPI/跨屏证据，完整目标继续进行；不新增Core接口、依赖、配置或crate，不暂存提交推送。

## 开源呈现路径参考与同步间隔实验（2026-10-03）

- 查阅 [OBS D3D11](https://github.com/obsproject/obs-studio/blob/master/libobs-d3d11/d3d11-subsystem.cpp)：等待句柄路径以 WaitForSingleObject(0) 检查就绪，Present 使用同步间隔1；[Win32CaptureSample](https://github.com/robmikh/Win32CaptureSample/blob/master/Win32CaptureSample/SimpleCapture.cpp) 的 FrameArrived 复制纹理后调用 Present1(1,0)。只参考行为并独立实验，不复制项目代码。OBS HWND flip-discard 与本项目透明 composition flip-sequential不同，不能整体移植。
- 本机安静Release成对实验，仅将 Surface.present 的 SyncInterval 从0改1，保留 DO_NOT_WAIT、等待句柄、效果算法与调度；不照搬参考项目的阻塞 flags0。官方 [Present文档](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-present) 表明同步间隔是呈现队列策略参数，因此以源版本与系统统计测量，而非单看GPU耗时。
- SyncInterval0：有效更新59.634FPS，GPU P95 2.649ms，OS更新间隔P95 27.784ms，dequeue→sync上界P95 26.007200ms，submit→sync上界20.052100ms，1,789延迟样本（target/annotation-sync-interval-zero-baseline.log）。
- SyncInterval1：有效更新57.368FPS，GPU P95 2.604ms，OS更新间隔P95 34.729ms，dequeue→sync上界P95 38.545300ms，submit→sync上界22.432800ms，1,721延迟样本，invalid0，模式端点匹配（target/annotation-sync-interval-one-experiment.log）。两个原生夹具各35s显式通过，但性能门槛没有通过；一次成对结果不能证明所有硬件的普遍行为。
- 本机结果不支持采用间隔1，已将唯一生产改动恢复为0；没有保留实验开关、依赖或配置。恢复后的Release冻结版本像素回归session5484已exit0：4个源画面版本、2种效果、2次拒绝提交，指定24点最大通道误差0；git diff --check通过。画质采样不替代全帧或实机显示，本批尚不声称20ms更新或50ms输入到显示达成。后续审计逐帧DirectComposition/窗口属性提交成本与源发布节奏，不重复已失败的重试或效果提前计算实验。

## 逐帧 DirectComposition 提交实验与桌面像素证据（2026-10-03）

- 源码确认 Surface 的 visual/root 在 attach 时绑定并Commit；规格中的input HWND/bounds变化会重建Surface。每次成功DXGI Present仍调用DComp.Commit与SetWindowPos。官方 [Commit](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-commit) 用于提交待处理的合成命令，[SetContent](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositionvisual-setcontent) 允许绑定IDXGISwapChain1；据此仅实验省略后续每帧Commit，保留attach提交、Present0/DO_NOT_WAIT、位置/层级、算法和调度。
- Release高模糊35s实验显式通过：OS有效更新57.900FPS、GPU P95 3.400ms、OS更新间隔P95仍27.784ms，dequeue→sync上界P95 31.302000ms、submit→sync 19.881000ms，1,737延迟样本（target/annotation-no-frame-dcomp-commit-experiment.log）。与近期间隔0基线对比没有更新间隔收益，不把不同运行的GPU波动视为因果结论；20ms/50ms目标仍未达成。
- 新增ignored原生桌面probe：拥有非激活输入窗口和128×128合成Surface，连续4次提交不同颜色，并用GetDC(NULL)/GetPixel读取桌面中心采样点，而非GPU后备缓冲。DC每次释放，窗口/Surface作用域销毁。无逐帧Commit版本4次颜色精确匹配（target/annotation-no-frame-commit-desktop-pixels.log）；初次夹具把RGBA纹理输入当成BGRA预期，修正通道预期后通过，没有容差放宽。
- 该证据说明本机指定场景可更新桌面合成采样点，不证明全帧、跨屏、真实App、资源全覆盖、物理发光或输入到显示延迟。因无明确性能收益，逐帧Commit已恢复，不保留实验开关。恢复后同一原生probe通过，最终workspace469/42、Clippy -D warnings、Windows Release及diff check通过；串行session2436已exit0，Release1m11s。日志 target/annotation-frame-commit-restored-desktop-pixels.log 与 target/annotation-desktop-pixel-probe-{workspace,clippy,release}-final.log。

## 原生提交 CPU 调用分段（2026-10-03）

- 仅测试诊断增加Surface每次成功提交的DXGI Present、DComp.Commit、SetWindowPos耗时；每次调用先清零，Renderer仅汇总接受的实际提交，避免拒绝候选复用旧计时。报告明确为接受attempt跨Surface的CPU调用总和，不是GPU或系统队列延迟；没有样本时unavailable。生产调用顺序、策略和依赖没有变化。
- 原生Release高模糊35.31s显式通过（target/annotation-submission-cpu-stages-large-blur.log）：2,246个提交调用样本，Present P95 0.100600ms、Commit 0.010300ms、SetWindowPos 0.073900ms。各阶段独立百分位不能相加当作总P95；跨Surface总和也不能解释单屏呈现。
- 同次OS更新59.802FPS、GPU P95 3.219ms、更新间隔P95仍27.784ms；1,794个版本关联样本，dequeue→sync上界P95 25.917200ms、submit completion→sync 19.950700ms。直接CPU调用成本小，不能据此认定重复窗口调用是主要瓶颈，也不能排除这些调用对系统调度的间接影响。input→visible仍未测，没有宣称性能改善。
- 初次编译修正submission_p95索引的usize标注；运行报告确认非空合法cohort。最终workspace469/42、Clippy workspace/all-targets -D warnings、Windows Release及diff check均通过，串行session8634已exit0，日志 target/annotation-submission-stages-{workspace,clippy,release}-final.log。下一步针对源发布与呈现刷新周期关联继续测量；只在实际证据支持时改变调度或窗口策略，完整结构与实机目标继续保留。

## 参数通知所有权与过期兜底清理（2026-10-03）

- 面板程序回填不入队后，on_values_changed 仍保留旧的“借用失败就根据共享绑定重建Target”逻辑；该路径与新回填规则不一致，缺失/退休Registry时还可能构造无效请求。现在只尝试读取一次有效会话/面板目标，没有目标则只吸收通知基线、不排队。保留实际执行时的完整Target校验及InputWork退休取消。
- 观察基线迁入annotation_parameters::Notifications，封装投影吸收和用户字段Patch生成；参数模块不读取Registry或持有窗口。去掉回调捕获的对象、编辑、watermark、projection兜底绑定。普通对象binding仍由原State负责，没有扩大到其他事件协议。
- 不再被兜底路径共享的watermark/projection修订号从Rc<Cell<u64>>改为State拥有的u64，保留checked_add与原身份失效规则。编辑State去掉重复identity Rc<Cell<Option<u64>>>，当前编号直接由active Instance.edit提供。生命周期回归改查真实活动实例编号；Drop仍通过弱组件失效、退休定时器与回调断言验证，没有隐藏窗口常驻。
- 107项annotation相关回归通过（target/annotation-notification-owner-regression.log），覆盖排队线宽/菜单颜色合并、退休面板拒绝旧请求、序号放置默认目标、文字角点事务、编辑实例替换与水印输入。最终workspace/Clippy/Windows Release串行session92159运行中，日志 target/annotation-notification-owner-{workspace,clippy,release}-final.log；须核对终态后补记。未改Slint外观、Core接口、GPU算法或调度，无新增依赖/配置/crate，无实机IME/跨屏截图；不暂存提交推送。


## 参数等待队列与回放规则所有权（2026-10-03）

- 参数模块新增 Pending，拥有按原 Target 合并、读取、分批取出和清理的规则；Registry 不再直接操作请求 Vec。相同目标替换保留首次接受顺序，取出批次后新请求不混入当前回放。
- ReplayContext 将当前目标、普通面板工具、绑定对象和序号取消事实解析为 Current / SequenceDefaults / Discard。协调器只提供当前事实并调用应用入口，取消恢复的旧选择不会成为请求的新目标。
- 保留 Patch、通知基线、应用入口二次校验及 Core 撤销协议；未改变 GPU、界面、公开接口、配置或依赖。
- 最终相关回归 107 通过，包含序号完成/取消、面板退休、字段合并、笔刷滚轮及马赛克长滑杆/取消等实际 UI 回调测试。日志 target/annotation-pending-replay-regression.log。
- 完整门禁 session 3649 已 exit 0：workspace 469 通过/42 忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release 和 diff check 均通过。日志 target/annotation-pending-replay-{workspace,clippy,release}-final.log。该证据不代表 GPU 性能目标、真实 App/IME/跨屏已验收。


## 参数到几何事务的统一交接（2026-10-03）

- Registry::begin_document_gesture 在几何快照开始前完成参数事务，序号放置、对象移动/缩放和非编辑文字角点均接入；启动入口不再隐式依赖上游按下路由已完成参数事务。
- Gesture::commit/cancel 以 owns_document_transaction 统一保护 Core 完成/回滚。编辑器几何、选择及其他不拥有快照的手势不能结束参数历史。
- activate_mouse_mode 移除 set_mode 已完成后额外无条件 cancel_drag 和重复清理，保留模式切换拥有者的取消顺序。
- 新回归 geometry_handoff_retires_parameter_idle_without_merging_undo 使用真实 canvas_pointer 按下/移动/松开路径，覆盖 DPI1/1.25、负坐标、完成/取消及关闭面板，推进虚拟时钟600ms；证明参数闲置任务不提前结束几何，参数与移动仍独立撤销。已显式通过。初始夹具点位落在空心矩形内部未命中对象，改为边线点位；未修改生产命中规则。
- 第一轮107项相关回归通过；新增回归修正一个size字段类型错误后通过。最终完整门禁session9615已exit0：workspace470通过/42忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release与diff check通过（target/annotation-gesture-handoff-{workspace,clippy,release}-final.log）。GPU调度及实机验收未在本批完成。


## 长轨迹缓存命中跳过边界重算（2026-10-03）

- 新增显式 ignored Release CPU 探针 long_trajectory_signature_and_cache_costs，一条可见且不变的铅笔/一个视口，1000/20000/100000点，各240样本；分别记录签名、对象比较、clone+drop、缓存场景组装P95及点数据载荷。排除采集/GPU/提交/显示，不能作为端到端帧率/延迟证据。
- 发现 ordinary_document 在缓存比较前调用 content_bounds，后者调用 stroke_points 复制普通轨迹或生成曲线。将完整对象、DPI、参考文字相等检查提前，视口仍在槽键中；命中复用已有栅格及边界，变化仍重算，视口退出和对象删除继续释放缓存。未新增Core接口或修订号。
- 安静Release基线/优化分别session94921与16686 exit0。100000点缓存场景P95由0.210300ms到0.058200ms（约72%下降），20000点0.068700到0.011000ms。避免缓存命中时每次复制100000点的800000字节点载荷；这不是进程内存释放测量。签名P95为0.398500/0.432900ms，未优化也不宣称改善。两个日志target/annotation-trajectory-cpu-{baseline,cache-fastpath}.log。
- 结果为当前机器孤立CPU场景，不能推广到真实App帧率/跨屏/输入法或GPU显示改善。最终门禁session46720已exit0：workspace470通过/43忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release与diff check通过（target/annotation-scene-cache-{workspace,clippy,release}-final.log）。新增忽略项为上述已显式运行两次的CPU性能探针，不代表其他原生忽略项已验收。


## WGC 时间戳与系统同步关联的时序验证（2026-10-03）

- 补充测试专用capture_to_sync_ms：WGC SystemRelativeTime 的100ns单位与DXGI原始QPC计数按frequency转换，在i128整数域先相减，避免长uptime浮点相减失精度；拒绝零/负值、未来同步和刷新计数不一致。不改变生产采集/呈现策略。
- 新增长uptime/非10MHz频率单位回归，扩展精确源版本关联回归。WGC时间晚于已确认dequeue时只拒绝补充时钟指标，保留原有有效dequeue→sync/submit→sync样本；无有效值报告unavailable，不能夹成零或删掉失败后报达标。
- 安静Release高模糊：72378/88397/78132均exit0。首次1781个源版本时钟全拒绝，最后1785个全拒绝。最后运行OS59.502FPS、GPU P952.645ms、系统更新间隔P9527.784ms，未达20ms；没有input→display证据。日志target/annotation-capture-clock-{large-blur,investigation,direct-qpc}.log。
- 独立取帧QPC验证原始样本：frequency10000000、WGC时间1616526277113、取帧QPC1616526169458，相差+10.7655ms；另外两例+10.2344ms、+10.5440ms。证据说明这批报告时间不能作为已发生的取帧前起点；不认定为普遍WGC缺陷，不用该时间戳代替paint/input/photon。取帧QPC、限量三例及关联全为cfg(test)，未增加生产热路径日志或公开接口。
- 官方依据：SystemRelativeTime描述compositor QPC time，TimeSpan.Duration为100ns（https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.systemrelativetime、https://learn.microsoft.com/en-us/uwp/api/windows.foundation.timespan）。微软示例仓库issue127为用户对窗口内容更新与DWM时间差异的报告，未获官方结论：https://github.com/microsoft/Windows.UI.Composition-Win32-Samples/issues/127。
- 下一步应使用自有源画面可读取版本标记与绘制时钟关联，不能猜测最近paint或把本批诊断当作延迟优化。最终门禁session8357已exit0：workspace471通过/43忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release与diff check通过（target/annotation-capture-clock-{workspace,clippy,release}-final.log）。全部时钟仍不提供input→display证明。


## 像素确认的自有源绘制到系统同步关联（2026-10-03）

- SourceFrame::paint记录单次BitBlt开始到GdiFlush完成的区间。像素夹具每种效果的四个色带版本只发布一次；拒绝候选时注入109，后续复用该次出版，避免相同像素重复绘制导致无法识别起点。捕获仍按24点颜色识别准确版本，冻结源元数据到提交的映射不使用最近时间猜测。全部为cfg(test)。
- 成功提交的原生Count映射到已验证paint版本/绘制区间；后续OS统计只按完全相同Count关联，以系统同步时钟区间及绘制区间生成耗时范围。新增纯函数回归拒绝颠倒区间、绘制晚于同步及不匹配刷新计数。
- 初始试验在第一帧阻塞等待OS计数，35223/78853失败；具体证据expected=1、原生计数/时间为0，出现DXGI HRESULT0x887A000B。改为批量收集后续确认，不阻塞源推进；未放宽像素误差或用提交替代系统确认。
- 最终显式Release原生夹具64882 exit0，0.87秒：4版本×2效果、2次拒绝、每帧24点、最大误差0。8成功提交中7个取得匹配时钟，首次未确认如实保留。已匹配绘制到OS同步范围约39.39–66.32ms，仅为有阻塞全纹理读回及人为拒绝的直接Renderer夹具；不能当作生产worker、真实input→display、FPS或photon验收。日志target/annotation-paint-clock-batched-native.log。
- 下一步在持续运行的worker夹具中采用非阻塞的版本探针，版本不能在256帧后重复，并单独报告探针成本；保持正式未测项和20ms/50ms目标。完整门禁75459已exit0：workspace472通过/43忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release与diff check通过（target/annotation-paint-clock-{workspace,clippy,release}-final.log）。整体性能与真实App验收仍未完成。


## 持续 worker 非阻塞源版本探针（2026-10-03）

- 新增cfg(test) paint_probe：源版本最多四个待完成槽，复用2×1 staging纹理；只复制自有源第一/末尾色带各一个像素，Map使用DO_NOT_WAIT，不调用Flush、不阻塞等待，满槽或格式/解码错误明确计数。样本与CPU记录上限16384，worker销毁时释放GPU槽。探针错误只记诊断，不触发效果失败/设备重建。
- 自有夹具色带改为24位单调paint ID加band XOR校验，禁止溢出；不再每256帧重复。RGBA/BGRA及255/256、65535/65536、最大24位和损坏解码回归通过。此为测试画面编码，不改变用户标注或生产采集参数。
- 可选probe token只在新增显式 ignored持续性能夹具启用。准确Published(token,sequence,ticks)与解码paint关联，再按完全相同native Count连接OS同步；不以最近绘制时间或WGC时间猜测。PaintPublication记录BitBlt/GdiFlush范围，warmup/缺失/多Surface/混合/重复版本单独处理。
- 安静Release enabled20236 exit0：1800解码、1794paint→sync样本、2缺失、0歧义、0探针丢弃/无效/待完成；绘制到同步P95下/上40.356700/40.641200ms，轮询CPU P950.001300ms，OS59.833FPS/GPU2.766ms/间隔27.784ms。日志target/annotation-paint-probe-large-blur.log。轮询指标不包括enqueue复制成本；GPU计时覆盖该instrumented路径，不能宣称无开销。
- 同编码disabled对照16783 exit0：OS59.703FPS/GPU2.648ms/间隔27.784ms，未见明显吞吐退化，单次对照非因果证明。相同批次24点冻结版本像素回归4版本/2效果/2拒绝，最大误差0（target/annotation-paint-probe-{disabled-baseline,pixels}.log）。两像素只证明身份，不替代全帧质量、真实App/input/photon/跨屏验收。40.64ms不代表input→display达标，20ms间隔仍失败。
- 初始完整门禁55854止于Clippy的collapsible_if，已收拢条件且不抑制lint；最终完整门禁44804已exit0：workspace473通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release与diff check通过（target/annotation-paint-probe-{workspace,clippy,release}-final.log）。本批新增忽略项为已经显式运行的持续probe夹具。下一步将同一paint的真实CPU取帧、处理、提交及同步分段，而非继续用不可用WGC起点。


## 同一像素源版本的等待分段与三缓冲实验（2026-10-03）

- paint_stages将像素确认paint、该Published版本的真实CPU取帧Event、精确native commit和OS同步关联，验证publication≤dequeue≤commit≤sync。逐个样本生成paint→dequeue区间、dequeue→commit、commit→sync区间和paint→sync总区间；分别统计P95，禁止加减不同分布的P95作为总耗时。扩展原时钟回归验证阶段顺序及反向时序拒绝。全为cfg(test)。
- 双缓冲安静Release83912 exit0：1747同源样本，paint→dequeue上界P9515.065400ms、dequeue→commit9.576100ms、commit→sync上界20.251900ms，总上界40.781900ms；GPU2.588ms、OS58.270FPS、间隔27.784ms。日志target/annotation-paint-stages-large-blur.log。系统呈现等待较大，但非物理input/photon延迟证明。
- 查阅开源dcomp-overlay的原生代码（https://raw.githubusercontent.com/ntkrnlmp/dcomp-overlay/main/src/overlay_win32.cpp）：使用2缓冲/FLIP_SEQUENTIAL/PREMULTIPLIED及device级latency1，但未启用waitable flag。微软waitable指南指定SwapChain2方法；当前实现已有waitable+SwapChain2 latency1，未盲目复制device级设置。DXGI_PRESENT_RESTART文档限制flip/fullscreen，不用于本标注窗口。来源：https://learn.microsoft.com/en-us/windows/uwp/gaming/reduce-latency-with-dxgi-1-3-swap-chains、https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-present。
- 单因素BufferCount2→3实验28493 exit0：1780样本、GPU2.649ms、OS59.367FPS、interval27.784ms、commit→sync上界P9520.088000ms、总上界40.777300ms，无明确延迟收益。该对照不证明FPS波动由buffer数量导致，未采用。依据多swapchain建议仅用于实验，不宣称单Surface推荐3：https://learn.microsoft.com/en-us/windows/win32/direct3ddxgi/dxgi-multiple-swap-chains。日志target/annotation-buffer-three-experiment.log。
- 已恢复BufferCount2，并逐字核对surface.rs与实验前快照完全一致；未保留实验开关。最终完整门禁52889执行中，待核对。20ms仍未达，50ms真实input→display仍未测。


## 序号放置状态与终止行为所有权（2026-10-03）

- 根据修订计划开始手势迁移。新增 UI 内部 SequencePlacement，拥有按下起点及 Core 分配身份后的起始对象；插入、按起点更新、松开提交和取消均在手势模块内。协调器保留参数事务交接、面板投影、选择和编号同步。路由用 PlaceSequence 标记不再重复保存起点/目标，通用 gesture_object 不再保存新序号快照。
- 松开消费放置状态后回放参数，再选中新对象；取消消费状态并恢复 Core 快照后，沿用既有 Target/Pending 回放规则。关闭会话释放放置状态。未新增 Core 接口、依赖或配置。
- 检查发现第二次按下可沿用未完成放置的快照；现先取消旧放置再接受新按下。已有生产路径回归扩展至重入按下和关闭中放置的清理，仍覆盖 DPI1/1.25、负坐标、Escape、捕获丢失、工具切换、编号与选中恢复。
- 新增纯领域交互回归验证拖回起点、完成/取消与历史独立、编号耗尽插入失败释放事务。首次6项序号回归86234 exit0；标注模块109通过/1忽略33660 exit0；最终中断生产路径显式通过。日志 target/annotation-sequence-owner-{tests,final-tests,interruption}.log。
- 最终全量门禁40523已exit0：workspace474通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release及diff check通过。日志 target/annotation-sequence-owner-{workspace,clippy,release}-final.log。真实 App/IME/跨屏截图、窗口焦点迁移、其他手势和完整 GPU 性能验收仍未完成。


## 普通对象移动/缩放所有权与效果预览接线（2026-10-03）

- 新增 UI 内部 GeometryDrag：拥有 Core 对象身份对应的按下快照和 Move/Resize 操作数据，统一捕获、更新、完成和取消。Move 路由标记不再保存起点/目标；Resize 路由仅保留光标所需的控制点。实际对象定位、文字点击转入编辑及马赛克预览均从拥有者读取，不再拼接通用快照和路由目标。
- 马赛克 submit接收 GeometryDrag 的不可变引用，沿用对象身份、起始几何、效果上下文和 remask 规则；cancel_pending 使用相同拥有者定位临时对象。窗口关闭释放所有者，第二次按下先取消未完成对象事务。参数交接、目标/字段回放、文字角点及输入恢复协议保持。
- 通用 gesture_object 剩余调用只服务文字角点，已改名 text_handle_original；关闭会话时释放它。未新增 Core公开接口、依赖或配置。
- 复用生产输入、参数交接、文档命令、马赛克临时对象和光标回归；迁移原纯测试验证真实 GeometryDrag 的对象身份、一次撤销及铅笔拒绝。新增缩放回归验证按下快照、拖回原边界、DPI1/1.25及取消无额外历史。初次80482 exit0，标注109通过/1忽略；最终55959 exit0，110通过/1忽略。日志 target/annotation-geometry-owner-{tests,final-tests}.log。
- 全量门禁73760已exit0：workspace475通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release及diff check通过，日志 target/annotation-geometry-owner-{workspace,clippy,release}-final.log。文字角点所有权与剩余手势、窗口焦点、GPU等待、完整画质/资源及真实App验收仍未完成。


## 文字角点按下状态、阈值与终止所有权（2026-10-03）

- 新增 UI 内部 TextHandle：拥有起点、抓取偏移、原始对象、字号DPI、编辑/文档目标及阈值激活状态。目标位置与对象变换在手势模块内生成，协调器仅负责当前已接受参数回读、参数/Core事务交接及将结果写回编辑实例或文档。路由标记只保存光标与参数阻塞所需的handle/editing/active；Registry不再独立存放text_handle_original。
- 激活前允许参数更新；超过3×DPI后重新读取当前对象作为快照，编辑草稿不取得Core撤销事务，非编辑对象独立取得事务。完成和取消消费真正拥有者；移除可凭复制路由标记结束Core事务的通用commit/cancel。再次按下和关闭释放文字控制操作，原有按钮删除/连接箭头、旋转锚点及焦点恢复保留。
- 发现并修复未激活取消回写旧编辑快照的问题：未发生几何修改时不回写旧对象，保留已接受字号。扩展既有真实编辑实例回归，在未超过阈值时改22→33并取消；旧取消规则的隔离复现85366 exit101，确实回退22，恢复修复后同批110通过/1忽略exit0。临时规则只改变cancel返回条件，恢复后与实验前文件逐字一致；未保留实验开关。日志 target/annotation-text-handle-inactive-cancel-negative.log、annotation-text-handle-owner-final-tests.log。
- 首次迁移95165 exit0，110通过/1忽略；现有回归继续覆盖DPI1/1.25、负坐标、抓取偏移、阈值前参数、草稿与Core历史分离、取消及连续参数回放。最终全量门禁47375已exit0：workspace475通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release（2m22s）及diff check通过；日志 target/annotation-text-handle-owner-{workspace,clippy,release}-final.log。
- 剩余绘制/折线/连接箭头及统一活动状态、窗口焦点生命周期、GPU等待、完整画质/资源与真实App/IME/跨屏验收仍未完成；受控回归不代替实际交互截图。


## 绘制按下参数、折线状态与草稿构建边界（2026-10-03）

- 新增 UI 内部 annotation_drawing 模块。Press拥有起点、Kind、按下样式/DPI及箭头最大移动距离，路由Draw只保存工具类型；绘制更新通过借用的已接受参数视图和草稿槽工作，不读取Registry，不新增每次移动的整份参数复制。马赛克的绘制上下文使用Press起点，非绘制对象仍使用GeometryDrag；首帧采集失败、切换箭头工具和取消时退休Press。
- Polyline拥有确认对象与最后点击时刻；节点追加、预览、退格、双击判断及完成最小点数在同一所有者内，Registry移除last_polyline_click。ArrowPending将原有按下设置和有效终点判断迁入该模块，保留两次点击绘制与3×DPI阈值。区域、笔刷、普通对象及放大镜草稿构建共用纯函数，UI仅提供参数来源。
- Enter完成折线、退格删除最后一点时退休PlacePoint，修复迟到松开再建折线。扩展生产canvas_pointer回归，覆盖DPI1/1.25、负坐标、按下期间快捷键、迟到松开、仅确认节点提交和一次撤销；新增纯Polyline回归覆盖350ms边界、退格重置双击、填充/非填充点数及预览不提交。
- 首次仅拆分所有者85888 exit0，110标注回归通过/1忽略。Press迁移1142因两个测试仍使用旧Draw字段编译失败，已迁移对应所有者并清理无用导入；修复后63914 exit0，111通过/1忽略。最终迟到松开生产回归通过，日志 target/annotation-drawing-owner-late-release.log；完整门禁20170已exit0：workspace476通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release（2m20s）及diff check通过；日志 target/annotation-drawing-owner-{workspace,clippy,release}-final.log。
- 绘制结果展示草稿仍由会话协调器提供给场景；统一活动状态的互斥与路由入口尚待收拢，不把本批等同于阶段1全部完成。窗口焦点、CPU热路径、GPU等待、完整画质/资源及真实App/IME/跨屏验收保持。


## 活动手势互斥所有权与派生路由（2026-10-03）

- Registry的gesture改为Option<Active>，Active互斥持有SequencePlacement、GeometryDrag、TextHandle、绘制Press或纯动作。移除四个并列拥有者槽；路由Gesture从当前拥有者即时派生，文字阈值激活后无需同步第二份标记。Action仅能表示正文选择、控制按钮、逐点放置等无文档事务动作，不能伪装为移动/缩放/放置拥有者。
- 创建、更新、按类型取出完成与取消均接入该槽。错误类型的take保留原槽，取消只执行实际拥有者的Core/编辑恢复；旧路由副本仅用于参数回放。画布、编辑转发、光标与马赛克预览统一读取派生路由或借用的拥有者，不重新存储目标。第二次按下、采集失败、工具切换及会话关闭使用同一退休入口/槽。
- 初始迁移20606编译失败：两个测试仍写旧字段、一个测试读旧路由字段、两处需要分开借用手势/Session；全部修复。90411 exit0，111标注回归通过/1忽略。新增错类型完成保留拥有者、纯动作取消不结束连续参数历史、实际几何取消恢复文档回归，显式通过（target/annotation-active-owner-terminal-tests.log）。
- 首轮完整2619止于Clippy的测试专用Active导入，未进入Release；已用cfg(test)限定导入，不抑制lint。最终全量17205已exit0：workspace477通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release（2m20s）及diff check通过。日志 target/annotation-active-owner-{workspace,clippy,release}-accepted.log。
- 多次点击等待（箭头/折线）与显示草稿仍分别存于Registry；其互斥与展示生命周期需要继续明确。窗口焦点、CPU热路径、GPU等待、完整画质/资源及真实App/IME/跨屏验证保持原范围，本批不宣称完整目标完成。


## 多次点击等待互斥与显示草稿清理（2026-10-03）

Registry 的箭头/折线并列槽迁为 Option<drawing::Pending>；typed view 仅提供实际类型的引用。无效箭头终点不消费等待，错误类型完成保持实际拥有者；移出画布时折线显示已确认节点，箭头保持最后预览。切换工具/分组选项及统一取消清理同一槽；删除统一取消之后不可达的两段重复 Escape 分支。光标入口改为派生视图，修复 annotation_cursor.rs:151 E0615。

新增 pending_completion_preserves_other_kinds_and_invalid_endpoints，覆盖 DPI1/1.25、负坐标、无效到有效终点、错误类型完成、预览与已确认节点。保留生产画布快捷键、迟到松开、捕获丢失及一次撤销回归。首轮相关测试47951 exit0；简化取消后最终完整门禁60772 exit0，workspace478通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release和diff check通过。日志：target/annotation-pending-owner-{workspace,clippy,release}-final.log。

本批未修改 GPU 调度，也未执行真实App/IME/双屏截图验收。下一批窗口关闭原因与菜单返回/焦点拥有者迁移；完整CPU/GPU性能、20ms更新间隔/50ms真实输入显示、画质和资源范围继续保留。


## 菜单关闭原因与输入归还策略（2026-10-03）

annotation_choice::close 必须接收 CloseReason：InteractionFinished、Replaced、ControlPressed、OwnerRetired。同按钮收起、选择/取消完成、显示失败和外部关闭按交互结束归还输入；弹层替换、点击其他控件保留控件输入状态，不调用 release_parameter_input。面板/水印所属窗口销毁、目标/位置变化和语言刷新按退休处理，清理状态但不请求焦点。take 退休菜单的任务后先取消旧恢复请求，只有合法交互完成再请求新的恢复。关闭面板不再依赖“菜单先恢复，面板最后取消”决定行为。

新增实际回调回归 popup_replacement_and_control_press_keep_input_until_interaction_finishes，验证替换不释放输入、工具条按下保持输入、交互完成只归还一次、面板销毁不聚焦退休控件、旧弹层回调无效。相关3698 exit0，114通过/1忽略。完整19561 exit0：workspace479通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m17s、diff check通过。日志 target/annotation-close-reason-{tests,workspace-final,clippy-final,release-final}.log。

这是窗口/焦点阶段的首个行为批次，并非整个阶段完成。下一批审计菜单真实返回所有者：水印返回目前直接 invoke_focus_input，普通文字使用可取消的延迟恢复，应明确各自身份与任务校验；保留预设模板追加、输入法及既有焦点策略。B/C剩余迁移、CPU热路径、GPU等待、20ms间隔/50ms真实输入显示、完整画质/资源及真实App/IME/跨屏截图仍待完成。本批未修改 Slint 界面、Core接口或 GPU 调度，不暂存/提交/推送。


## 菜单返回编辑实例与共用焦点校验（2026-10-03）

focus::Request 以 Target::Text{editor,panel} 或 Target::Watermark{editor} 绑定真正的输入实例，菜单版本及会话作为共用条件。focus::Context 仅提供必要的活状态摘要；匹配规则移入焦点模块，不读取 Registry。所有恢复均检查新弹层及正在输入/拖动的控件；普通文字额外检查面板身份和水印窗口优先级，水印不依赖工具条实例身份。Coordinator 保存最新请求并支持按目标取消；关闭水印窗口停止其待恢复任务。

菜单关闭按实际所属窗口选择返回目标；水印不释放无关面板输入，finish_return 删除直接 focus_input 的旁路。restore_editor_focus 在操作结束后选择当前水印输入或普通文字实例；水印只调用原有内部 focus_input，不新增原生激活动作，普通文字保留既有激活/光标重置行为。

新增生产回调/受控时钟回归 watermark_menu_returns_to_its_owner_and_retirement_cancels_restoration：预设追加、菜单关闭后的延迟归还、面板更换、控件输入期间不恢复、控件结束返回水印、新弹层/错误会话/错误输入类型拒绝、保留旧组件的关闭重开与任务取消。共用 timer 回归增加跨目标取消不影响另一请求。序号说明回归新增水印打开期间拒绝旧文字请求，关闭后仍在原选区输入。

初步完整85374门禁通过后追加优先级复现；62764 exit101 证明旧匹配会越过水印输入（target/annotation-focus-watermark-priority-negative.log）。首个替换脚本因格式未匹配断言失败，未应用修复；随后补齐实际条件。最终66245相关115通过/1忽略；完整27468 exit0：workspace480通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m20s、diff check通过。最终日志 target/annotation-focus-target-{tests,workspace,clippy,release}-accepted.log，以 accepted 批次为当前源码证据。

下一批审计外部/前台变化关闭时的激活策略：当前普通文字仍沿用激活请求，原生 DismissRequested 无关闭原因，需区分局部输入恢复与明确用户激活，防止跨应用抢焦点；不以受控 Slint 测试宣称真实 App 该项已验收。窗口/菜单剩余迁移、CPU热路径、GPU等待、20ms更新间隔/50ms真实输入显示、完整画质/资源和真实App/IME/跨屏截图仍保持原范围。本批不修改Core接口、配置、依赖或GPU调度，不暂存/提交/推送。


## 原生关闭原因与局部焦点恢复（2026-10-03）

现有 PopupDismissMonitor 保留原来的鼠标/前台监控机制，私有窗口消息携带 PointerOutside 或 ForegroundChanged，桥接和 App 映射保留原因到 UI。没有新增 Core 公开接口、crate、配置或依赖。翻译语言菜单、托盘菜单、翻译弹窗继续处理两种关闭事件。标注三级菜单和工具分组菜单仅对 PointerOutside 使用按钮/菜单位置防误关判断；前台变化关闭不再因光标停在弹层上而被忽略。二级参数面板继续不注册自动关闭，收到任意原因均不退休。

focus::Request 增加 Activation::Local/UserRequested。外部关闭与菜单准备失败恢复内部输入，不调用原生激活；明确交互结束保持现有普通文字激活。水印仍只内部 focus_input。菜单关闭原因中补充 ExternalDismissed/PresentationFailed，替换、控件按下和所属窗口退休规则不变。

新增实际 Slint 回调/受控时钟回归 foreground_dismissal_closes_the_menu_without_activating_the_text_window：鼠标停在三级菜单上仍关闭，二级保留、编辑身份和选区不变，随后输入替换原选区，原生激活计数保持0；明确菜单选择后激活一次。扩展二级/分组菜单回归覆盖两种关闭原因，保留DPI1/1.25及旧窗口事件拒绝。Platform 回归直接执行私有消息的真实 subclass 分支，检查两种原因及未知编码拒绝；它不等于实际 HWND 消息/全局钩子/Alt+Tab实机验收。App回归验证原因映射。

相关2466 exit0：UI116通过/1忽略、Platform18通过/1忽略、App映射1通过。最终57978 exit0：workspace483通过/44忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release2m15s、diff check通过。日志 target/annotation-dismiss-cause-{ui-tests-final,platform-tests-final,app-tests-final,workspace-final,clippy-final,release-final}.log。

本批关闭了外部 dismissal 路径上的原生激活，不宣称跨应用焦点全部验收：明确操作结束请求排队后发生前台切换的竞争仍需审计来源和执行时授权，不能只用当前编辑身份替代。真实App的Alt+Tab、IME、双屏、负坐标和截图仍待执行。下一批继续该请求竞争/窗口任务来源边界，随后推进场景CPU热路径与GPU等待；20ms更新间隔/50ms真实输入显示、完整画质/资源与全范围实机验收保持。本批未修改GPU调度，不暂存/提交/推送。


## 参数输入确认与失焦完成分离（2026-10-04）

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


## 工具变体与会话退休统一取消（2026-10-04）

本批收拢 annotation.rs 的同组子工具切换和会话关闭。原 set_mode 在工具组不变时提前返回，菜单随后只清理箭头捕获及草稿；普通形状和铅笔的 DrawingPress 可能继续存活，迟到松开仍按旧操作提交。select_tool_variant 现在在变体改变时先调用所属操作取消，保留参数回放时的原目标，再结束编辑、退休面板和更新工具。重复选择同一变体不额外取消。原菜单会话/面板/组及菜单类型身份校验保留。

set_mode 删除取消入口之后的重复箭头/马赛克草稿清理。会话 close 在原生释放前递增 generation，使排队输入失效；直接消费 Active.cancel 和 Pending.cancel，不回放参数、不恢复即将销毁的编辑窗口，然后按原资源路径释放。马赛克首次采集失败不再直接清空 gesture/draft，改走统一取消，避免绕过操作拥有者。

区域/笔刷属于马赛克参数模式，切换只影响后续创建，已松开的待采集对象不是活动捕获；本批没有改变这项语义。切换离开马赛克工具仍取消待验证对象，新增回归验证活动移动、待验证遮罩与无关参数历史同时存在时，不留下对象/移动历史，也不吞掉无关参数撤销。

既有受控 canvas 回归扩展到同组子工具切换及直接关闭，覆盖形状/铅笔、100%/125% DPI、负坐标和迟到松开。定向 annotation 筛选191通过/3忽略（annotation-input-retirement-focused.log）。完整门禁87433 exit0：workspace498通过/47忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、git diff --check；UI build.rs检查Slint。日志 target/annotation-input-retirement-{workspace,clippy,release}.log。没有新配置、依赖、Core公开接口、GPU或布局改动；未暂存、提交、推送。受控事件不是实际 App 的捕获/IME/跨屏证据，无新实机截图。

下一批处理剩余多次点击悬停预览与显示草稿写入：annotation.rs 仍在 PointerLeft 与移动分支识别 Polyline/Arrow、直接创建/清空 draft。迁移到已有 Pending 拥有者，保持确认节点、起始DPI、冻结参数及迟到事件规则，协调器只消费明确刷新结果。其余 GPU 来源新鲜度/系统同步、CPU副本、实际App焦点/IME/DPI/双屏、完整画质和资源继续保留；20ms间隔未达标、50ms真实输入显示未测，整体目标未完成。


## 多次点击指针预览与移出刷新（2026-10-04）

Pending 新增 pointer_moved，接受必要 draft/point/scale 及可选预期 Kind。折线点击捕获仅请求 Polyline、箭头点击捕获仅请求 Arrow；悬停没有预期类型，可更新当前等待。错误类型返回 false 且保留草稿。annotation.rs 三处移动路径不再直接生成/写入折线或箭头草稿，由等待拥有者处理。起始箭头参数保持冻结，折线 tentative-node 的2×viewport DPI阈值保持；没有新增 Core 接口、配置或依赖。

审计发现 PointerLeft 原来恢复确认节点后，仅在 hovered 非空时请求刷新；没有悬停对象时可能继续显示离开前的末端预览。本批 pointer_left 返回实际显示变更，协调器在预览变化或hover清除时排队刷新。已是确认草稿则不再克隆和刷新；箭头保持最后预览并返回 false。比较发生在移出事件，不在每次移动新增完整对象比较。

新增纯回归覆盖100%/125%scale、负坐标、两种等待类型、错误捕获请求保持草稿、悬停不确认节点、临近节点阈值、跨viewport DPI不重设箭头样式、折线恢复确认显示及重复移出无变化。既有受控canvas回归新增移动预览→移出→检查frame_tasks.render_pending，hover为空仍必须刷新；重复移出不排队，文档与撤销保持空。该证据证明生产事件路由与任务调度，不是实际App跨屏截图或物理显示验证。

定向最终5426 exit0：annotation192通过/3忽略（annotation-pending-preview-focused-final.log）；初轮69533也通过但尚未包括最终受控移出断言。完整门禁71064 exit0：workspace499通过/47忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、git diff --check；UI build.rs检查Slint。日志 target/annotation-pending-preview-{workspace,clippy,release}.log。没有实际App截图、GPU或视觉布局改动，未暂存、提交、推送。

Registry仍持有显示draft，马赛克按下初始化仍经过Registry桥接；折线预览仍复制确认轨迹，不能据此宣称CPU性能改善或草稿所有权全部迁移完成。下一批测量长折线持续预览及长笔刷更新的复制/几何成本，优先用已有拥有者和共享场景边界优化实测热路径；保留完成对象的冻结语义与失效完整性。GPU来源新鲜度/系统同步、20ms间隔、50ms真实输入显示、窗口/IME/焦点历史/DPI/双屏/负坐标、完整画质和资源均继续保留。


## 长轨迹预览复用与重复边界扫描（2026-10-04）

Release隔离CPU基准使用1k/20k/100k交替Y坐标轨迹，DPI1.25，每项240样本。首次94760编译失败：Bounds没有Default；改为from_corners后77626通过。原生产路径基线：100k折线预览P95 0.3271ms、马赛克追加0.1894ms、独立bounds扫描0.0329ms。范围不含采集、场景编译、栅格化、GPU或显示。

Pending的折线显示现在通过display_polyline复用现有draft.points容量，并从确认对象复制完整字段与节点；仅临时端点超出2×viewport DPI阈值时，扩展已确认bounds，不再扫描全部节点。移出恢复确认节点也复用该缓冲。确认对象仍不可变，节点不降采样；完成/取消消费draft，未新增跨会话缓存。原polyline_preview及便捷preview仅保留cfg(test)独立旧路径，用于几何与成本对照。Object字段逐一构造，新增字段会要求编译时补齐，不依赖部分字段更新。

马赛克append_point本身已经refresh_bounds；UI删除其后的第二次全量扫描。新单点对象必须先refresh以计入radius：append_point忽略重复起点，不做初始刷新会损坏单击圆形边界。本批保留Core追加/简化算法，未改Core公开接口、配置或依赖。

新增回归：100%/125%scale、负坐标、多方向临时端点、阈值内恢复，逐次完整Object对照旧clone+scan；连续100次移动及移出恢复保持points地址。笔刷单点、重复/近距离采样、共线末端简化、拐角和反向扩展逐次对照旧append+额外refresh。初轮90452一条断言失败：最后输入在确认节点附近，pointer_left应返回false；修正断言并额外移到远端后检查true，生产代码没有为测试改变语义。最终定向98639中的annotation193通过/4忽略。

首轮优化性能98639通过，但两份轨迹先克隆、固定顺序测量的夹具出现20k笔刷new0.0422ms/old0.0152ms，未接受为性能结论。修正为各自输入立即准备、奇偶样本交替先后顺序，逐样本比较两份笔刷结果。55363配对成本通过，29530先复测同一最终代码再执行完整门禁。配对P95(ms)：

| 节点数 | 折线旧→新（首测） | 折线旧→新（复测） | 笔刷旧→新（首测） | 笔刷旧→新（复测） |
|---|---|---|---|---|
| 1k | 0.0018→0.0004 | 0.0043→0.0002 | 0.0024→0.0016 | 0.0037→0.0041 |
| 20k | 0.0776→0.0049 | 0.0763→0.0061 | 0.0485→0.0412 | 0.0454→0.0393 |
| 100k | 0.2690→0.0163 | 0.2887→0.0139 | 0.2245→0.1800 | 0.2001→0.1638 |

长轨迹收益在两轮中一致；1k笔刷没有稳定收益，未为小数量另建算法分支。初次跨轮基线不作为因果前后数字，结论采用上述同进程旧/新配对；保留原顺序有偏差的日志。轨迹仍完整复制，Core仍扫描追加后的bounds；不声称所有遍历消失、进程内存下降或实时显示达标。

最终29530 exit0：workspace500通过/48忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、git diff --check；UI build.rs检查Slint。日志target/annotation-drawing-preview-{baseline,baseline-fixed,focused,focused-final,cost,cost-paired,cost-repeat,workspace,clippy,release}.log。Release基准为显式ignored测试，新增忽略不掩盖失败；无实际App截图或GPU改动，未暂存、提交、推送。

下一批回到GPU来源新鲜度与系统同步诊断，继续以相同像素版本分离采集/许可/处理/提交/同步，避免用CPU局部收益解释未解决的27.784ms间隔。Registry剩余草稿初始化、文字/窗口实机焦点/IME/DPI/双屏/负坐标、完整画质/资源、20ms间隔与50ms真实输入显示要求继续保留，整体目标未完成。


## 同像素版本的来源内容新鲜度（2026-10-04）

本批仅修改cfg(test) performance诊断，没有改变生产采集、效果、调度或呈现。已有画面probe解码paint ID并关联冻结来源sequence/ticks及精确交换链提交；新增paint_freshness在测试结束后的报告阶段统计该paint之后、截至取帧/提交/OS同步边界已完成发布的新版数量。发布区间after<=边界才计数，同步区间分别产生下/上界；current fraction上下界反向采用newer上/下界。拒绝缺失paint、倒序阶段或无效publication区间，不猜测WGC时钟、事件到达或物理显示。没有在采集/worker热路径扫描发布表。

首轮编译失败：既有paint ID为u32，新增helper误写u64；按既有类型修正，不新增转换层。纯回归覆盖同版本、旧ID排除、发布区间与同步区间重叠、恰好边界、缺失ID及倒序/无效时间。2673包含修正后的纯回归和35秒Release自有WGC源测试，正常exit0；原性能报告先复制保留为target/annotation-performance-paint-latency-before-freshness-时间戳.txt。

35秒测试有1789同像素版本匹配样本，freshness invalid0；paint_sync missing2、ambiguous0。结果：

| 阶段 | 已完全发布的新版本数P95 | 当前仍最新比例 |
|---|---|---|
| 取帧 | 0 | 99.5528% |
| 提交 | 1 | 58.0771% |
| OS同步 | 下/上界均2 | 下/上界均0% |

同版本取帧到提交P95 9.8399ms；提交到OS同步上下界20.1663/20.1664ms；整个发布到同步上界40.5772ms。各阶段P95不能相加。GPU effects P95 2.067ms，系统有效59.670版本/s、间隔P95 27.784ms，20ms目标仍不通过。WGC时间与QPC直接转换仍unavailable，没有借新统计声称时钟问题解决或真实输入显示已测。

这些数据支持优先审计取帧后的比较/处理/输出确认及OS呈现等待，没有证明采集完全无问题，更不代表所有App/硬件。当前自有源中大多数取帧已是最新已发布内容，不能据此把采集API替换作为已确认优化方向。后续应把确认事件关联到冻结候选身份，再测取帧→采集比较确认→效果提交→输出确认/原生提交的同版本等待；目前Confirm事件使用Source当前sequence，不能未经审计直接当成冻结candidate对应时序。

查阅开源Win32CaptureSample的SimpleCapture.cpp：CreateFreeThreaded/FrameArrived→TryGetNextFrame→CopyResource→Present1，关闭session与framepool；它不执行本项目的效果合成、采集比较及输出候选确认，不能直接用其策略证明本项目的性能。参考https://github.com/robmikh/Win32CaptureSample/blob/master/Win32CaptureSample/SimpleCapture.cpp。官方TryGetNextFrame仅承诺取下一帧或空池返回null，不保证“最新”：https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.trygetnextframe。本批未复制或引入外部依赖。

最终1771 exit0：workspace501通过/48忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、git diff --check；UI build.rs检查Slint。日志target/annotation-paint-freshness-{tests,tests-fixed,live,workspace,clippy,release}.log。没有新实际App截图、配置、Core公开接口、依赖、生产性能改动；未暂存、提交、推送。下一批做冻结候选的同版本阶段归因，再以独立实验决定优化。剩余Registry草稿初始化、窗口/IME/焦点历史/DPI/双屏/负坐标、完整画质/资源、20ms间隔与50ms真实输入显示全部保留，整体目标未完成。


## 冻结采集候选的确认身份与阶段关联（2026-10-04）

Source.confirm此前用当前sequence记录事件；比较期间若latest变为下一帧，会把确认标成后来的编号，而实际publish_candidate仍发布冻结候选。本批在发布消费之前保存candidate(token/sequence/ticks)及比较started时间，将Confirm事件明确绑定该候选，外层sequence也使用候选编号。128事件边界与丢弃计数保持；所有改动仍cfg(test)，没有生产采集/比较/调度变化。

报告的Confirmations按完整Published身份建立索引，拒绝跨来源或started晚于event.at；相同身份出现多次确认标为ambiguous，不挑首个/最近记录。阶段匹配必须dequeue<=started<=confirmed<=commit，并只用于已通过paint ID、冻结来源和OS交换链提交匹配的样本。缺失/歧义单独记录，不让新索引丢掉既有整段延迟与新鲜度样本。

加强Source回归：frame200开始比较后frame300出现，confirm仍携带sequence2/ticks200，published也为200；比较取消和发布策略继续原行为。新增报告纯回归覆盖外层sequence999仍按candidate2匹配、不同token/sequence/ticks、倒序dequeue/commit、重复确认与无效事件。首轮编译失败，phases需要Published而遍历得到&Published；修正为显式解引用，不改变任何版本选择策略。

修正后82928 exit0：confirmation筛选2通过，published_version1通过；35秒Release自有WGC源实测正常退出。1792同paint版本样本，capture_phase缺失/歧义0、confirmation invalid0、freshness invalid0；paint_sync missing2/ambiguous0。阶段P95(ms)：

| 阶段 | P95 |
|---|---|
| 取帧到比较开始 | 0.0039 |
| 比较开始到确认事件 | 3.8907 |
| 确认到原生提交 | 5.7101 |
| 取帧到提交（完整同版本） | 9.1470 |
| 提交到OS同步上下界 | 20.1918 / 20.1919 |

各阶段P95不可相加。比较的3.8907ms是GPU队列/执行/CPU轮询的总观察时间，不是比较shader active time；确认后5.7101ms也包含处理/输出确认/原生提交等待，尚未拆清。取帧时97.1540%最新，提交64.3415%最新，本轮源/系统相位有波动，不能拿上一轮99.55%直接当成优化或回退。GPU effects P95 2.693ms，OS59.768版本/s、间隔27.784ms；20ms目标仍未通过，真实输入到显示未测。

最终59820 exit0：workspace502通过/48忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、git diff --check；UI build.rs检查Slint。日志target/annotation-candidate-phases-{tests,tests-fixed,published-tests,live,workspace,clippy,release}.log。旧实测报告先备份target/annotation-performance-paint-before-candidate-phases-时间戳.txt。没有实际App截图、生产性能修改、新配置/依赖/Core公开接口；未暂存、提交、推送。

下一批单独测d3d_compare内核的GPU执行与结果观察，使用现有计时能力、同设备/尺寸/区域/改变比例，避免把3.89ms直接归为shader成本。若比较内核存在实测瓶颈，再对线程组/归约进行单因素对照并核验精确变化区域；确认到提交及OS呈现仍需继续独立测量。所有剩余草稿/窗口实机焦点/IME/DPI/双屏/负坐标、完整画质/资源、20ms间隔和50ms真实输入显示范围保持，整体目标未完成。


## 像素比较线程组的隔离GPU测量与16×16归约（2026-10-04）

新增显式ignored的gpu_comparison_thread_group_costs：同一个Backend/device/context、2560×1440 RGBA纹理、完整ROI，比较相同像素/右下角单通道变化/整屏变化。每组10轮热身后240样本，奇偶轮交换8×8/16×16先后。复用已有gpu_timing::Profiler，从dispatch清空标记前到16字节readback复制后记录GPU timestamp，CPU观察包含query/result轮询及100us请求sleep的系统调度。不是纯shader active time、不含WGC采集、源纹理更新和物理显示；每次严格对照Equal或精确Changed(Rect)，disjoint/dropped为0。

首轮编译缺少测试局部D3D11/DXGI导入及p95函数，补齐后81351正常退出。隔离配对结果促使实验16×16：相同0.095808→0.057824ms、单点0.096000→0.079552ms、整屏变化0.146368→0.100992ms。CPU观察约1.2ms，没有一致改善，不能把这段GPU成本当成上一批3.89ms源确认等待的全部成本。

生产Kernel现在固定GROUP=16；shader与Dispatch由同一GROUP值生成，编译字符串仅在创建kernel时发生。实验8×8入口仅cfg(test)，不新增配置或Core接口。RGBA精确比较、圆角/遮罩策略、region范围、结果16字节、非阻塞Map、候选发布/拒绝协议均不变。16×16减少线程组和全局extent归约次数，仍保留每像素比较，不近似采样。

原gpu_comparison_is_exact回归扩展到8/16两种组，13×7非整组边缘、所有RGBA通道、ROI外变化、扩大ROI、分離区域extent合并与旧baseline保留均通过。32826的gpu_comparison筛选把该回归与成本测试并行运行，故那次成本数据不作为独立复测证据；31356单独成本复测通过，P95(ms)：

| 像素变化 | GPU8→16 | CPU观察8→16 |
|---|---|---|
| 相同 | 0.095776→0.076000 | 1.1906→1.1918 |
| 右下角单点 | 0.096416→0.079360 | 1.1923→1.2075 |
| 整屏 | 0.146592→0.099424 | 1.2115→1.2300 |

两轮隔离GPU成本趋势一致；CPU观察没有收益。本机收益不保证其他GPU，完整实时跟随目标仍未通过。32826的35秒实时源测试1792阶段样本、缺失/歧义0，OS59.770版本/s、间隔27.784ms；比较开始到确认P95 3.7663ms、确认到提交5.5412ms、完整取帧到提交8.8111ms、提交到同步上界20.1386ms、整段发布到同步上界40.9016ms。和前轮之间相位/负载变化，不能把细微阶段差异直接归为线程组收益，各阶段P95不可相加。

版本画面夹具31356正常退出，4来源版本/2效果/2次候选拒绝、每帧24点最大通道误差0；阻塞读回夹具的paint_sync较大，不作为worker吞吐或输入延迟证据。31356中的拒绝提交筛选没有--ignored，仅0通过/1忽略，不算实际执行。该首轮完整门禁workspace通过、Clippy在基准chunks_exact_mut固定大小lint处失败，后续Release未运行；改为as_chunks_mut::<4>()，不压制lint。

最终77438先显式运行--ignored的拒绝提交回归1通过、8/16边缘回归1通过，再跑完整门禁：workspace502通过/49忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、diff check exit0；UI build.rs检查Slint。日志target/annotation-comparison-group-{costs,costs-fixed,hardware,live,costs-serial,pixels,rejection,rejection-accepted,edges-accepted,workspace,workspace-accepted,clippy,clippy-accepted,release-accepted}.log。实测前保留旧报告target/annotation-performance-before-group16-时间戳.txt。未暂存/提交/推送，无新配置/依赖/Core公开接口或实际App截图。

下一批回到真实采集/输出队列：隔离batch GPU成本约0.10ms，不能推导负载中的源比较active时间，也不能从3.77ms观察时间减去0.10ms就声称队列等待。需要把实际source比较的GPU timestamp关联到冻结候选，并分离copy/dispatch/readback完成、轮询观察以及确认后输出处理。所有剩余草稿/窗口/IME/焦点历史/DPI/双屏/负坐标、完整画质/资源、20ms间隔与50ms真实输入显示范围继续保留，整体目标未完成。


## 实际采集候选的GPU时序与查询扰动对照（2026-10-04）

已有gpu_timing移为windows内部共用cfg(test)模块，文件仍位于annotation_gpu/gpu_timing.rs，由windows/mod.rs声明、呈现引用；没有复制查询实现。旧GPU查询硬件夹具改为直接创建D3D纹理/RTV并clear，去除对私有Graphics/Texture呈现包装的依赖。生产版本不编译该设施。模块收拢首轮脚本因错误匹配Backend字段可见性中断，只保存了前两项修改；随后补齐Backend/Source，并修正cfg赋值块、相对测试路径，最终编译通过。未通过的compile/compile-fixed日志保留。

自有paint probe请求可额外选择采集计时；选择仅存在测试Mailbox，不是软件配置或Core API。Backend仅为目标HWND开启Source计时；其他源不插查询。GPU标记位于候选CopyResource之前、copy之后、比较dispatch和16字节readback之后，使用已有disjoint/frequency/单调性校验。CPU比较开始记录仍在copy调用之后，原确认身份协议不变。每个Source最多一组未退休计时，新的比较可无计时继续，不让新帧或cancel重绑定旧查询；query未准备好不等待、不额外flush，不推迟采集。时钟invalid、skip单独记录；GPU事件按完整token/sequence/ticks关联已识别paint/提交版本，重复、跨来源、非有限/负跨度均拒绝。

新增纯索引回归与hardware gpu_timing_identity_survives_cancel_and_later_frames；后者确认cancel和新frame之后旧查询仍对应原candidate、skip标记对应新帧、查询可复用且不发布任何采集。既有timestamp_queries_are_bounded_nonblocking_and_reusable显式执行通过。43447定向timing9通过/2忽略，身份硬件1通过、查询复用1通过，首次35秒源计时正常exit0，1788匹配样本、缺失/invalid/skip均0；copy GPU跨度P95 0.057504ms、compare/readback 0.109600ms、查询观察2.0571ms，CPU确认观察2.0705ms。GPU跨度不是纯shader active time或绝对队列等待。

初次CPU确认观察明显低于前轮约3.77ms，故新增查询关闭的校准，保留同样paint/呈现/GPU效果探针；不会用两轮P95相减估计队列。12672最终代码纯索引1通过，并串行运行查询off/on各35秒，均正常退出：

| 项目 | 采集查询off | 采集查询on |
|---|---|---|
| 同paint阶段样本 | 1791 | 1785 |
| capture GPU匹配 | 0（requested=false） | 1785，missing/invalid/skip0 |
| copy GPU跨度P95(ms) | unavailable | 0.060160 |
| compare/readback GPU跨度P95(ms) | unavailable | 0.113216 |
| 查询结果观察P95(ms) | unavailable | 2.1099 |
| CPU比较开始到确认P95(ms) | 3.7477 | 2.1188 |
| 确认到提交P95(ms) | 5.7732 | 5.7324 |
| 完整取帧到提交P95(ms) | 9.0179 | 7.9482 |
| 发布到OS同步上界P95(ms) | 40.5151 | 39.9922 |
| OS有效版本/s | 59.734 | 59.533 |
| OS更新间隔P95(ms) | 27.784 | 27.784 |

off报告的source_gpu_missing_or_ambiguous=1791仅表示这些paint没有GPU记录，requested=false时是预期，不是计时开启后的健康缺失。on没有缺失。查询可能改变命令提交/驱动调度，序贯两轮也有相位/负载波动；不能宣布生产获得约1.6ms提升，更不能把GPU跨度从CPU观察P95相减得到队列等待。两次on的GPU跨度相近仅证明带仪器场景的局部测量。27.784ms间隔仍未达标，真实输入/物理显示未测。

首轮52475 workspace通过、Clippy因Source方法对crate暴露更窄Trace类型失败，Release未运行。start_gpu/finish_gpu/poll_gpu收窄为pub(super)仅采集内部使用，没有扩大Trace可见性或压制lint。最终9330 exit0：workspace503通过/51忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release、git diff --check；UI build.rs检查Slint。日志target/annotation-source-gpu-timing-{compile,compile-fixed,compile-final,tests,identity,query-reuse,index-final,live,calibration-off,calibration-on,workspace,workspace-accepted,clippy,clippy-accepted,release-accepted}.log。原报告先备份before-source-gpu-timing/first时间戳文件；无实际App截图、新配置/依赖/Core公开接口或生产计时/调度改动，未暂存/提交/推送。

下一批研究最小完成查询的独立实验：保持采集GPU timestamp关闭，使用事件完成信号与非阻塞结果读取做对照，区别查询提交行为与昂贵计时仪器。不预先把它作为生产优化，要求精确像素/候选拒绝/采集失败保留与完整门禁，并比较同代码查询off的系统有效更新与整段时序。确认后输出等待、剩余草稿/窗口/IME/焦点历史/DPI/双屏/负坐标、完整画质/资源、20ms间隔与50ms真实输入显示范围均继续保留，整体目标未完成。


## 绘制按下初始化与完成/取消的同一拥有者（2026-10-04）

- Press::begin 现在负责退休旧显示草稿及马赛克按下预览；普通绘制保留原有移动/松开时创建预览规则。MosaicArea/MosaicBrush 使用同一 Press::update 创建初始对象，固定按下位置、参数和创建 DPI。协调器只读取参数并安装捕获，不直接清空/构造马赛克草稿。
- 原 Registry update_draft 包装仅留给历史回归夹具，标记 cfg(test)；生产按下、移动、松开、取消统一经 Press 方法。共享草稿存储仍在 Registry，本项不等同于完整草稿所有权迁移。
- 新回归覆盖负坐标、1/1.25 DPI、旧箭头预览退休、马赛克单点初始边界、参数改变及跨 DPI 松开不覆盖按下设置、单点铅笔及取消不新增撤销。已有真实画布事件回归 interrupted_drawing_cannot_commit_after_tool_switch_escape_or_capture_loss 通过（受控 Slint 事件，不是实际 App/物理屏幕验收）。
- 首次编译因新测试将 AnnotationValues.size 整数字段写成浮点常量失败，修正测试字段类型后重跑通过，未放宽断言。
- 门禁 session 69128 正常 exit0：workspace 504通过/51忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（2m16s）、git diff --check 通过。日志 target/annotation-draft-begin-{tests,tests-fixed,interruption,workspace,clippy,release}.log。ignored 不视为通过。
- 本批未调整 GPU、窗口布局、Core 接口、配置、依赖；未暂存、提交或推送。无新增实机截图、IME/双屏/跨应用证据。后续继续收拢 Pending 与显示草稿的共同退休及存储所有权，并推进独立 GPU 等待实验与资源验收。


## 多次点击绘制与预览的共同所有权（2026-10-04）

- PendingState 私有持有 Pending 操作与显示 preview。Registry 不再分别保存/改写多次点击操作与预览，开始等待箭头、鼠标预览、移出、落点、退格、完成、取消全部由该拥有者更新。Pending 枚举限定在 drawing 模块内部；内部几何与终止算法继续复用，没有改变箭头阈值或折线双击规则。
- display_draft 只借用捕获草稿和 PendingState 两个字段；帧签名、CPU/GPU场景、放大镜视图、采集区域、对象遍历与对应UI回归全部读取这份视图。避免借用整个Registry阻碍独立场景缓存更新。等待有效箭头端点且暂无预览时不会回退显示旧捕获草稿。
- 新 owned_pending_preview 回归覆盖负坐标、1/1.25 DPI、错误类型终止不破坏预览、移出仅保留已确认节点、悬停点不提交、完成/退格清空/取消共同退休、迟到松开不创建对象与一次撤销。原 pending_ 五项回归通过；完整workspace包含画布事件、参数及绘制中断回归。
- 最终门禁 session33448 正常exit0：workspace505通过/51忽略；Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（2m17s）、git diff --check通过。日志 target/annotation-pending-owner-{tests,lifecycle,workspace,clippy,release}.log。ignored 不作为通过。
- 普通捕获草稿仍由Registry存储并交给Press；本批不宣称完整草稿迁移或GPU/真实App验收完成。未新增Core接口、配置或依赖；未暂存、提交或推送。没有新增实际App截图、IME/双屏/物理显示证据。


## 普通捕获草稿随手势拥有与退休（2026-10-04）

- Press 私有持有 preview；begin/update/release/cancel 不再接收外部可写草稿。取消或丢弃捕获自动释放所属预览，取出捕获后Registry无法继续显示它。删除Registry.draft字段、初始化和清理分支，以及旧测试专用Registry update_draft包装。Active.cancel不再要求独立草稿参数。
- 统一只读草稿入口根据Active绘制捕获和PendingState返回预览，所有场景、签名、采集、放大镜、光标及采集失败判断迁入。渲染借用只限相关字段，不复制整份草稿，也不把整个Registry传给绘制算法。
- 原直接注入草稿的取消、马赛克投影、采集超时夹具改为通过Press::begin/update构建合法拥有者；保留原对象、参数、取消及撤销断言。绘制纯回归9通过/1忽略；新增captured_preview_follows_only_its_live_owner覆盖Rectangle/Pencil/MosaicBrush/Magnifier、负坐标、1/1.25DPI、错误类型完成保留原拥有者、取出/取消立即停止显示、取消不改变文档撤销。
- 首次编译发现annotation_cursor中的多行Registry.draft遗漏读取，补齐统一视图后重新编译验证。未放宽断言。
- 最终门禁session25190正常exit0：workspace506通过/51忽略；Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（2m16s）、git diff --check通过。日志target/annotation-captured-owner-{tests,tests-fixed,lifecycle,workspace,clippy,release}.log。ignored不算通过。
- 草稿存储迁移已完成这两类绘制路径，不宣称整轮编辑/窗口/GPU/资源验收完成。此次未修改Core公开接口、配置、依赖、GPU算法；未暂存、提交或推送。无新增实际App/IME/双屏/物理显示截图与测量。


## 参数菜单组件回调与会话执行边界（2026-10-04）

- 审计确认 build_window 已使用展示参数快照、Presentation 已接收明确原生能力；本批保留这些既有边界，没有重复创建窗口基础设施。
- bind_parameter_input 仅接收组件、InputWork、字段、焦点来源读取器及事件接收器，不读取Registry。Custom/Style/Selection/Color/Dragging形成私有类型事件；窗口所有回调统一进入其InputWork FIFO。open_menu为接收器附加原会话/面板/菜单编号，原目标校验、参数补丁、返回父菜单和焦点规则保持。
- 实际Slint回调回归parameter_callback_binding_is_fifo_owned_and_captures_focus_origin：Registry被借用时所有回调只入队，执行时无重入；验证六种事件顺序与负载、选项触发时的Origin不会被排队后的前台变化替换；取消也按序传递；退休同时丢弃排队选择并禁用保留组件的后续回调。已有重开/过期菜单及颜色目标回归随workspace通过。
- 首次截图夹具编译因项目tiny-skia未启用PNG输出功能失败，改为现有PPM输出，无新增依赖或特性。最终PPM用已有Pillow转换PNG并实际查看：330×80描边面板九个色按钮完整、无裁剪。路径target/annotation-parameter-callback-render/parameter-callback-outline.{ppm,png}。这是受控Slint软件渲染，不是实际App/IME/双屏证据。
- 最终门禁session48598正常exit0：workspace507通过/51忽略；Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（2m17s）、git diff --check通过。日志target/annotation-parameter-callback-{tests,tests-fixed,workspace,clippy,release}.log；ignored不算通过。
- 未改Core接口、配置、依赖、菜单样式或GPU算法；未暂存、提交或推送。整轮窗口、编辑、资源及GPU性能验收尚未完成。


## 采集比较完成事件：对照、全源验证及生产接入（2026-10-04）

- 依据D3D11_QUERY_EVENT/GetData契约，在采集比较CopyResource(readback,flag)之后End同一查询；GetData使用DONOTFLUSH，S_FALSE或FALSE继续Pending，完成后仍以Map(READ,DO_NOT_WAIT)读取精确差异。查询不负责发布、承认来源或交换基线。官方依据：https://learn.microsoft.com/en-us/windows/win32/api/d3d11/ne-d3d11-d3d11_query 。
- 将既有GetData原始HRESULT读取抽到Windows内部d3d_query，共用于完成事件与测试时间戳，避免generated Result把S_FALSE当成功。每个采集Comparison持有一个复用查询，只在创建时分配；创建成功后才安装Comparison，失败不留下半初始化状态。输出候选比较保持无完成事件。
- 最初只在测试中对自有HWND启用，关闭采集时间戳，串行OFF1/ON1/ON2/OFF2各35秒：比较开始到确认P95为3.8397/2.1802/2.1814/3.8030ms，取帧到提交为8.582/6.801/6.803/8.605ms。同像素绘制到OS同步上界41.1652/40.0249/40.4963/40.9689ms；不能当输入到物理显示。OS间隔仍27.783–27.784ms，采样缺失1–2，非法确认0。
- 再验证生产全部采集源启用，并复测OFF3：ALL1/ALL2/OFF3确认P95为2.0797/2.0798/3.7711ms，取帧到提交6.289/6.674/9.129ms；同像素绘制到OS同步上界39.9007/38.9469/40.8630ms。ALL有效自有源OS显示57.837/59.436FPS，OFF3为59.300FPS；不声称FPS提高。所有OS间隔约27.784ms，20ms目标仍未达到。阶段P95不相加/相减解释GPU活跃或具体队列等待。
- 采用生产采集完成事件，原Map参考、仅自有源和全部源模式仅在测试内保留；没有用户配置/Core接口/新依赖。记录scope避免把不同模式混为同一基线。硬件精确比较验证8/16线程组×事件关闭/启用、RGBA边缘、ROI排除/扩展及基线复用；原像素夹具4版本×2效果、2次拒绝、每帧24采样点，max_channel_error=0；原单次来源变更在未就绪surface及拒绝提交后仍保留。
- 失败记录：BOOL位于当前windows crate的core模块而非Win32::Foundation，修正类型；字段格式化造成一次旧测试字段替换遗漏，补齐后重跑；像素夹具首次误用Debug触发其Release前提，随后明确用Release通过，没有修改夹具前提。
- 性能会话2113、8347和71837均正常收尾（71837因Debug像素前提失败停止，后续门禁未执行）；最终session45836正常exit0，显式Release像素/拒绝回归通过；workspace507通过/53忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release（1m10s）、git diff --check通过。UI/Slint源码未变，沿用前批当前源码的Slint构建证据。日志target/annotation-completion-event-{off1,on1,on2,off2,all,all-fixed,all2,off3,exact,exact-fixed,exact-final,pixels,pixels-release,rejection-release,workspace,clippy,release}.log，保存同名对照txt及all1.txt；ignored不作为通过。
- 本机对照证明确认观察/取帧到提交改善；其他GPU、真实App输入/IME、双屏DPI、物理显示及完整资源验收仍未完成。没有新增实际App截图；未暂存、提交或推送，整轮目标保持活动。


## 编辑输入工作队列的独立生命周期拥有者（2026-10-04）

- 审计确认State::take/clear已先使实例不可访问再调用stop并销毁组件；没有重复改写正常完成协议。补齐未安装TextEdit直接丢弃时的队列退休：InputLifetime私有持有Rc<InputWork>，其Drop禁用队列并停止update/finish计时。组件回调仅持有队列引用，无法使生命周期拥有者存活。TextEdit布局字段仍可按原方式移动用于提交，无额外clone或Core公开接口。
- 新回归uninstalled_editor_drop_retires_queued_and_retained_input覆盖构建中止前已排队update或finish、caret/preparation计时、直接Drop未显式stop、保留组件后迟到输入/完成/取消全部无效，最后组件释放后Weak队列无法升级。原正常take/replace/ownerDrop及输入合并回归随workspace通过。
- 门禁session69110正常exit0：workspace508通过/53忽略；Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（2m20s）、git diff --check通过。日志target/annotation-editor-input-lifetime-{tests,workspace,clippy,release}.log。ignored不算通过。
- 本批无界面外观/布局修改，没有新增实际App或软件渲染截图；受控队列引用证据不替代实际App IME/跨屏/窗口/GPU/DC资源验收。未修改配置、依赖、Core公开接口；未暂存、提交或推送。整轮目标仍未完成。


## 编辑原生鼠标接收器随实例退休（2026-10-04）

- 审计Platform popup：WM_NCDESTROY注销dismissal、清理resize、移除窗口属性和subclass并释放PopupInputBridge；本批未重写已存在的销毁逻辑。发现UI pointer_text只检查Weak组件存活，保留旧组件时实例已退休但仍可把原生事件送入旧Slint输入控件。
- pointer_sink由TextEdit创建并读取其输入工作活动标记；实例stop/销毁立即停止转发，即便组件/原生接收器尚存。仍使用Weak组件与原dispatch_pointer_input，不变DPI、旋转或视口坐标规则。删除协调器独立pointer_text接收器，show_text_editor从实际匹配实例获取sink。
- 新回归pointer_bridge_stops_dispatching_to_retained_retired_component实际通过Slint窗口分发Moved/LeftPressed/LeftReleased：活动时TouchArea回调非零；显式stop后不增加；新实例可正常转发；直接Drop后其接收器失效，旧接收器也不复活。受控软件窗口事件证明退休保护，不等同于实际App IME/跨屏/native销毁资源验收。
- 门禁session4406正常exit0：workspace509通过/53忽略；Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（2m16s）、git diff --check通过。日志target/annotation-editor-pointer-lifetime-{tests,workspace,clippy,release}.log。ignored不算通过。
- 未变UI外观、Core公开接口、平台通用机制、配置或依赖；没有新增截图，未暂存、提交或推送。完整目标及未达GPU显示间隔要求继续保留。


## 输出比较完成事件的独立对照与生产接入（2026-10-04）

- 只改变输出Comparison完成观察；采集侧固定已验证的全源事件模式，输出Map参考/事件ON顺序OFF1/ON1/ON2/OFF2各35秒，无并发构建。实验scope在报告中区分All与CaptureAndOutput，采集时间戳关闭。
- 同像素版本取帧到提交P95：6.382/5.341/4.802/6.695ms；绘制到OS同步上界40.4035/37.7395/34.7213/39.3146ms。OS显示FPS59.169/59.101/59.669/59.067，间隔P95均27.783–27.784ms；不宣称FPS提高或真实输入到物理显示改善。确认到提交分段P95为8.5954/8.4439/3.3108/5.1130ms，波动明显，不能用其差值归因某个等待。版本阶段匹配缺失/非法0；paint同步缺失1/1/2/1，samples1774/1772/1789/1771。
- 输出Comparison创建时一次分配完成事件并复用，enqueue在readback拷贝后End，轮询复用d3d_query DONOTFLUSH/BOOL检查及Map DO_NOT_WAIT；完成事件不读取/发布像素。原scene fence、候选stamp、不就绪surface、equal承认及成功提交后更新baseline逻辑未变。采纳生产默认；仅测试可关闭事件用于参考模式。
- session54542四轮正常退出；最终session71778正常exit0：Release原像素版本对照4版本×2效果、2拒绝、每帧24采样点最大通道误差0；原单次来源变更在未就绪surface/拒绝提交后保留回归通过。workspace509通过/54忽略，Clippy --workspace --all-targets -- -D warnings、Windows Release（1m10s）、git diff --check通过。UI/Slint源码未变，沿用当前源码前批编译证据。日志target/annotation-output-completion-{off1,on1,on2,off2,pixels,rejection,workspace,clippy,release}.log与四轮txt；ignored不算通过。
- 未新增Core公开接口、配置或依赖；没有新增实际App截图，未暂存、提交或推送。其余GPU/显示等待归因、完整资源/跨屏/IME及真实输入到物理显示验收继续保留。


## 原生GPU后端十轮重开与资源退休证据（2026-10-04）

- 审计采集队列：CAPTURE_BUFFERS=2，latest_pending有界消费最新帧；本批没有以未经证明的旧帧积压假设改写调度。新增显式Release原生生命周期回归，在固定owned source/input HWND下交替模糊/马赛克创建、实际呈现、clear、Drop十个后端，Drop同步join worker后再检查，场景Weak每轮均失效。
- 每轮固定250ms消息泵/采集关闭settle，预先排除前两轮驱动缓存初始化。十轮(handle,GDI,USER)全为(313,8,5)，没有增长；最后先销毁SourceFrame的选中位图/DC，再销毁两个OwnedWindow并pump，得到(313,5,2)。句柄门禁预先限定warm baseline+4背景波动，GUI门禁不许增长；本机实测无波动。不把句柄/GUI数量等同GPU驻留页、进程内存、完整App窗口或输入钩子。
- 首次夹具因640×360窗口不满足既有1280×720 SourceFrame固定采样尺寸失败，统一原夹具尺寸后通过，未改像素检查；初始门禁session17435 workspace通过，Clippy常量assert前提检查失败后停止，Release未执行。改为项目原有if cfg!(debug_assertions) panic模式，无lint allow；最终session64615再次显式硬件回归通过并正常exit0。
- 最终workspace509通过/55忽略、Clippy --workspace --all-targets -- -D warnings、Windows Release（无生产改动增量0.64s）、git diff --check通过。日志target/annotation-backend-lifecycle-{tests,tests-fixed,tests-final,tests-accepted,workspace,workspace-accepted,clippy,clippy-accepted,release-accepted}.log。ignored不算通过。
- 本批只新增原生后端资源证据与测试，未变生产路径、Core接口、依赖或配置；无实际App/IME/DPI/物理显示截图，未暂存、提交或推送。继续保留完整App与GPU纹理/内存验收、显示间隔及真实输入到显示目标。


## 效果缓存按场景需求退休（2026-10-04）

- 审计发现 Effects.plans 按 sigma 位模式插入但从未淘汰；update 的 used 集合还包含强度为 0 的遮罩，随后跳过计算，导致旧 tile 与输入/版本元数据继续持有。新增私有 active_mosaic 需求判定，非零强度与可见遮罩决定效果资源归属；暂停采样不取消归属。
- regions 与 update 都按当前场景保留实际使用的 Blur Plan，避免调用顺序影响退休。tile、inputs、versions、source_versions 及测试来源记录沿用同一有效 asset 集合淘汰。没有修改滤波算法、像素确认、采集节流、Core 接口或依赖。
- 纯逻辑回归覆盖七档强度、同 sigma 共用方案、暂停保留、零强度/马赛克不使用模糊方案、无可见遮罩和删除。显式 Release GPU 回归覆盖遮挡时像素及版本不变，强度归零时六类缓存为空，恢复后像素逐字节等于原输出且版本递增，切换马赛克时 blur workspace 退休，以及最终删除清空元数据。使用负坐标与创建 DPI 1.25；这是 owned GPU 夹具，不是实际 App/双屏验收。
- 显式 Release effects 测试三项全部通过（新增缓存回归、原放大镜采样/擦除/层序、原下层标注采样/上层清晰回归）。最终门禁 session52509 正常 exit0：workspace510通过/56忽略；Clippy --workspace --all-targets -- -D warnings、Windows Release（1m08s）、git diff --check通过。UI/Slint源码本批未变，保留前批当前源码的编译证据。日志 target/annotation-effect-cache-{gpu,effects,workspace,clippy,release}.log。忽略测试不计为通过，三项硬件回归另外显式执行。
- 资源持有集合退休有证据，但不等同于 GPU 驻留内存即时下降或整体跟随延迟改善；未新增实际 App 截图。完整 App、IME、跨屏及真实输入到物理显示验证继续保留。未暂存、提交或推送；完整目标仍活动。


## 原生输入退休先于会话资源拆除（2026-10-04）

- 先增强既有工具条退休回归：close 后保留旧组件和 sink，在 Registry 可变借用期间送入旧 Moved。改动前明确失败（session93622、exit101、annotation.rs main_current 的 RefCell already mutably borrowed），证明仅检查全局身份仍会进入退休输入路径。
- window::InputWork 新增私有模块内 pointer_sink，将实例活动检查放在原生回调最外层。主工具条、二级参数/工具菜单、水印、画布及独立三级菜单全部使用所属 InputWork 包装；文字编辑沿用已验证的自有输入活动检查。原身份验证、DPI dispatch、取消、弹层关闭与松开后焦点返回逻辑保留，不延迟普通鼠标分发。
- Slot、CanvasLayer、编辑 State 与菜单 State 提供所属输入退休操作；Registry.close 先使会话身份失效，再停用所有窗口输入及其显示/定位任务，随后执行既有手势取消、呈现器/采集清理和组件销毁。原正常 clear/Drop 复用拥有者退休操作；不使退休组件复活，不通过 try_borrow 忽略合法输入。
- 工具条回归额外在 annotation_magnifier_sync 清理回调内同步送入旧事件，回调实际执行一次且不 panic；画布替换和水印重开时也在持有 Registry 可变借用期间送旧事件。新增受控 Slint 回归覆盖 1.0/1.25 DPI 的真实 Moved/Pressed/Released 分发、仅停用输入但组件尚存、替换不复活旧 gate、直接 Drop 拒绝原生输入，以及最后 sink 释放后 Weak 输入队列失效。保留的旧 hook 可以持有已退休 gate，但不会构成引用循环。
- 修复后专门工具条、Slot、原水印点击/模板/重开回归通过；最终 session65260 正常 exit0：workspace511通过/56忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint 编译、Windows Release（2m16s）及 git diff --check通过。日志 target/annotation-pointer-retirement-{before,after,teardown,slot,watermark,workspace,clippy,release}.log。忽略测试不计通过。
- 此批为受控生命周期与 Slint 事件证据，不是实际 App 原生 teardown、IME、双屏或物理显示验收。没有外观改动或新增实际 App 截图；没有修改 Core 公开接口、配置、依赖或平台通用弹层规则；未暂存、提交或推送。GPU 等待、完整 App 及其余模块边界验收继续保留。


## 二级工具条控件输入与会话执行分离（2026-10-04）

- 从 open_panel 提取普通控件接线到 UI 内部 annotation_panel.rs。Input 明确表达输入确认/结束、打开水印、滑杆按下/松开、工具分组选项、三级菜单、Escape/窗口关闭和撤销。bind_input 只接收组件、所属 InputWork、焦点来源读取能力及事件接收器，不访问 Registry、Core 文档或事务。
- 每次控件回调先捕获实际用户操作的 Origin，再按原有单次 FIFO InputWork.post 投递；协调器 apply_panel_input 负责会话/面板/分组校验、参数事务收尾、菜单路由和输入焦点恢复。移除 open_panel 内被替代的七类 callback 与窗口 close 接线，不增加平行状态、额外事件循环等待或通用事件总线。原工具菜单选择限制、马赛克撤销入口和 KeepWindowShown 行为保持。
- values_changed 差量通知仍需在回调阶段读取实际参数目标和区分对象回填，保留既有 Notifications/目标身份协议；本批没有把它改成投递时重新绑定当前选中对象，也没有宣称所有协调器写入已迁移完毕。
- 新 Slint 绑定回归在持有 Registry 可变借用时实际调用九次控件 callback，验证接收器不内联执行、FIFO 顺序和输入 payload；投递前更换 Origin 后仍使用回调时来源；退休前已排队和退休后新输入都无效。workspace 同时通过现有工具切换、参数/撤销、焦点、二级面板保持展开、三级菜单和旧组件回归。
- 专门绑定回归通过，最终 session70720 正常 exit0：workspace512通过/56忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint 编译、Windows Release（2m15s）及 git diff --check通过。日志 target/annotation-panel-input-{binding,workspace,clippy,release}.log；忽略项不计通过。
- 本批不改外观、Core 公开接口、配置、依赖或 GPU 算法，没有新增实际 App 截图。真实 App/IME/双屏/物理显示及其余协调器边界验收仍保留；未暂存、提交或推送，整轮目标保持活动。


## 水印展示快照与所属输入接线（2026-10-04）

- 新增 UI 内部 annotation_watermark.rs：build_window 只接收原始模板值与物理位置快照，负责创建/主题/初值；bind_input 投递 Presets、DismissPresets、Apply 与 Cancel 私有事件；pointer_sink 复用实例活动 gate、弱组件及调用者提供的路由校验/菜单处理能力，同步分发保持不变。
- 协调器仍决定定位请求、显示重试/窗口所属、应用解析/撤销、关闭和会话/实例校验。移除旧水印创建/控件绑定与独立 pointer_watermark 路径。保留原定位规则、默认 Slash 24h 和原解析，不宣称本批解决屏幕工作区/DPI 定位问题。
- InputWork.receiver 复用三处相同的类型事件转 FIFO 投递逻辑，二级工具条、三级参数菜单和水印绑定共同使用；仅为所属队列的闭包适配，不增加事件总线、全局订阅、额外排队轮次或状态副本。
- 新回归在 Registry 可变借用期间构建窗口并调用四个实际 Slint 控件 callback，验证原始多行中文/时间模板不被构建阶段解析、默认预设、请求物理坐标、FIFO 和 payload，以及退休前排队/退休后输入被丢弃。原水印鼠标定位/中文键入/时间预设/应用/取消/保留旧组件重开回归通过（包含 1.0/1.25 DPI）。原二级控件/三级菜单绑定回归也在最终 workspace 验证新的共享 receiver。
- 夹具失败与修复：MinimalSoftwareWindow 不实现位置读写，初始位置断言得到(0,0)；添加遵循当前 Slint WindowAdapter 的位置记录适配，保留原坐标断言。第一次适配有 Rc::new_cyclic 类型推断及 SoftwareRenderer::new 参数错误，明确具体 Rc 类型并使用当前无参构造后通过。位置证据只证明请求坐标传递，不等于原生屏幕定位。
- 最终 session84747 正常 exit0：workspace513通过/56忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint 编译、Windows Release（2m16s）和 git diff --check通过。日志 target/annotation-watermark-input-{binding,binding-fixed,binding-accepted,pointer,workspace,clippy,release}.log；失败夹具记录保留，忽略项不计通过。
- 不改 Core 公开接口、配置、依赖、GPU 处理或 UI 外观；无新增实际 App 截图。实际 App/IME/跨屏资源与物理显示目标仍未完成；未暂存、提交或推送，整轮目标保持活动。


## 工具窗口物理尺寸定位与水印DPI校准（2026-10-04）

- 明确复现小工作区故障：旧水印代码以固定440物理像素计算max_x，300像素宽工作区导致 clamp(min=-100,max=-240) panic；session49928/exit101为实际失败。初始夹具误用无Default实现的生命周期类型，改用原new构造后才得到此复现，日志均保留。
- placement::place_toolbar_popup 收口固定尺寸工具窗口的上下偏好与工作区约束；公共 clamp_origin 使用i64中间计算，处理u32极端尺寸和i32坐标。原二级工具条panel_position委托同一规则，保留下方优先；水印优先上方。超过工作区尺寸时不擅自改变布局/尺寸，定位到工作区起点，不宣称超大窗口能够完整容纳。
- 水印构建先以工具条位置确定所属监视器；原生准备/显示后读取实际物理窗口尺寸与DPI，使用原15逻辑像素间距定位，删除固定440/270/285/65的物理尺寸计算。所属Slot另做两轮32ms校准，不激活窗口；每轮校验会话与水印实例，Slot退休取消定位任务。原激活仍只来自打开请求的Origin许可。
- 水印窗口显示或校准使native暂停采样区域变化时，沿用现有frame队列请求场景刷新；不直接在Registry借用内调用全局schedule_render，不改变GPU滤波/采集算法。其余弹层身份和关闭规则继续保留。
- 新几何回归覆盖100%/125%实际物理尺寸输入、负坐标、上方不足翻下、右边缘、小工作区、无工作区及极端坐标/尺寸；原菜单/二级工具条定位回归随workspace通过。小工作区窗口实际构建/显示夹具回归通过，并验证旧实例校准不查询新工作区、当前实例仍查询。受控软件窗口本身不记录原生位置，因此精确坐标由纯几何回归验证，不等于真实双屏定位。
- 专门placement过滤17项通过；最终源码门禁session99359正常exit0，专门小工作区/旧校准回归通过，workspace516通过/56忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint编译、Windows Release（2m16s）和git diff --check通过。日志 target/annotation-watermark-placement-{before,reproduced,tests,fixed,workspace,clippy,release}.log。忽略项不计通过。
- 无Core公开接口、配置或依赖修改；没有新增实际App定位截图。真实跨屏/IME/资源及输入到物理显示验收仍保留，未暂存、提交或推送；整轮目标保持活动。


### 编辑结束结果结算（2026-10-04）

当前批次将 ClosedEdit 的普通文字、已有文字及序号说明结算迁入 annotation_editor.rs::resolve，输出私有 EditChange（New/Text/Caption）。协调器保持会话与对象身份校验、Core Session 修改、工具条回填及画布路由；原结束入口全部调用方共用此结果，删除原内容/边界/草稿样式结算分支。序号空说明可清空，普通空白文字沿用原行为；目标消失不能转换成新对象。此批不修改 GPU 调度、Core 公开接口或焦点协议。

新增两项结算回归覆盖取消、缺失目标、空说明、已排版边界及草稿实际 RGB。首次测试编译因误用 Slint 结构体 setter 失败，改为字段赋值后两项通过。最终实际源码门禁55559 exit0：workspace518通过/56忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs 编译、Windows Release（2m27s）及 diff check 通过。日志 target/annotation-edit-settlement-{workspace,clippy,release}.log。

项目级规则与本批没有难以调和的冲突，AGENTS.md 保持原样。真实 App/IME/双屏/物理显示与完整资源验收未完成。下一步继续审计编辑结束的跨模块回填与参数入口，并独立归因 GPU 提交许可及系统同步等待，不以本批门禁宣称整轮完成。


### GPU 提交许可归因（2026-10-04）

在 cfg(test) Stages 中新增合成前/提交前许可检查结果，直接记录实际所有 surface 的 gate，区分未检查、允许、阻塞。PermitPolls 单独记录被 Source 标签覆盖的许可阻塞；不以 RenderWait 标签推断许可等待，不把检查次数当作等待时长。生产调度和交换链参数不变。纯回归覆盖未检查、允许及混合等待；原生单次来源变化/延迟 surface/拒绝提交回归增加实际 gate 断言并通过。

两轮35秒 Release 自有60Hz源大模糊实验（相同 CaptureAndOutput 完成查询配置）：

| 指标 | 第一次 | 第二次 |
|---|---:|---:|
| 合成前许可检查 | 2261 | 2192 |
| 合成前阻塞 | 0 | 1 |
| 提交前许可检查 | 2255 | 2187 |
| 提交前阻塞（含 Source 标签覆盖） | 0 | 0 |
| 来源对应 OS 计数 FPS | 59.667 | 58.735 |
| 来源对应系统 QPC 间隔 P95(ms) | 27.783 | 27.783 |
| 提交到系统同步上界 P95(ms) | 19.6887 | 19.9375 |
| paint 发布到系统同步上界 P95(ms) | 34.7137 | 35.6964 |

结论限于此机器/夹具：许可不是本轮主要阻塞来源，下一实验应关联采集、提交与系统合成时序；20ms间隔目标仍未达。未测物理显示、真实输入、跨屏、完整App；许可统计不证明所有场景无阻塞，也不是性能改善对照。

核对官方 DXGI 指南（https://learn.microsoft.com/en-us/windows/uwp/gaming/reduce-latency-with-dxgi-1-3-swap-chains）及 OBS D3D11 源码（https://github.com/obsproject/obs-studio/blob/master/libobs-d3d11/d3d11-subsystem.cpp）：OBS 非阻塞查询 waitable object，但使用普通 HWND CreateSwapChain；它的呈现参数与我们的透明 DirectComposition 链路不同。结合此前 SyncInterval1 未获稳定收益的实验，本批不照搬、不改变生产参数。

首次纯诊断编译因 RenderWait 非 Copy、从借用 Attempt 移动字段失败，改为借用后通过。最终实际源码门禁63151 exit0：workspace519通过/56忽略、Clippy、Slint 构建、Windows Release、diff check通过。日志 target/annotation-permit-{observation-1,observation-2,retention,workspace,clippy,release}.log，数值副本 target/annotation-permit-observation-{1,2}.txt。无项目级规则冲突，AGENTS.md 未修改。


### 重复窗口定位的独立对照与撤回（2026-10-04）

假设：每次呈现后的 SetWindowPos（位置/尺寸/Z序不变）可能造成系统合成工作。仅测试实验通过每个 Surface 的首次成功定位标志，首次仍定位显示，后续跳过定位；采集、比较、效果、Commit、交换链许可和其余参数保持一致。仅用于自有静止 HWND 的实验，不验证窗口层级被外部改变后的恢复能力。

以相同 CaptureAndOutput 配置按 ON1/OFF1/ON2/OFF2 各35秒交替测量（ON指跳过重复定位）：

| 指标 | ON1 | OFF1 | ON2 | OFF2 |
|---|---:|---:|---:|---:|
| 定位调用 CPU P95(ms) | 0 | 0.1483 | 0 | 0.1521 |
| 对应 OS 计数 FPS | 58.499 | 59.833 | 59.670 | 58.702 |
| 对应系统 QPC 间隔 P95(ms) | 27.783 | 27.783 | 27.783 | 27.783 |
| 提交到系统同步上界 P95(ms) | 18.9798 | 19.8125 | 20.3161 | 18.6863 |
| paint发布到系统同步上界 P95(ms) | 34.1512 | 34.6647 | 35.1360 | 34.7901 |

结论：重复定位 CPU 调用被消除，但系统显示间隔不改善，提交到系统同步变化不稳定，不能据此采用新的定位状态机。已将 annotation_gpu.rs、surface.rs、performance.rs 恢复到本实验前保存的精确字节，仅撤回本批实验，保留之前已验收的许可诊断及所有其他改动。不将本轮当作生产优化。

官方依据：SetWindowPos 改变窗口位置/尺寸/Z序，参数不变也不能推导一定无系统工作（https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos）。DXGI_FRAME_STATISTICS 的 PresentRefreshCount 是对应显示的刷新计数，SyncRefreshCount 是调度器QPC采样刷新计数（https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics）；现有诊断继续要求两者相同，不把查询时刻当作显示时刻。

日志与数据：target/annotation-placement-experiment-{on1,off1,on2,off2}.{log,txt}；实验前源码副本 target/annotation-placement-experiment-before-{0,1,2}.rs。下一步关联实际帧到达/提交/系统合成刷新相位，确定长间隔形成于提交前或提交后；仍保持20ms显示目标和完整App/IME/跨屏/资源/物理显示要求。

恢复后的实际源码门禁43160 exit0：workspace519通过/56忽略、Clippy --workspace --all-targets -- -D warnings、Slint构建、Windows Release及diff check通过。日志 target/annotation-placement-restored-{workspace,clippy,release}.log。


### 同像素版本的相邻帧间隔归因（2026-10-04）

新增仅测试 CadenceFrame/CadencePair：每帧端点是已解码 paint 的发布完成、该版本取帧、接受提交、对应 OS 同步的 CPU 时钟上界。比较同一相邻版本对在四个阶段的间隔；三项 signed growth 是相邻阶段间隔之差。对单个帧对严格满足 sync interval = publication interval + 三项 growth；绝不相加/相减不同分布的 P95。拒绝跨链、重复/倒退 paint或提交计数、非单调端点；源跳帧单独记录。到达通知没有逐帧身份，本批不将最近通知与像素版本猜测匹配。

两轮相同自有60Hz源、CaptureAndOutput 完成查询的大模糊 Release 实测：

| 指标 | 第一次 | 第二次 |
|---|---:|---:|
| 有效帧对/无效帧对 | 1792 / 0 | 1794 / 0 |
| 系统同步间隔 >20ms 帧对 | 584 | 615 |
| 长间隔中跳过 paint 的帧对 | 4 | 3 |
| 长间隔最大扩大处：发布→取帧 | 444 | 447 |
| 长间隔最大扩大处：取帧→提交 | 41 | 85 |
| 长间隔最大扩大处：提交→同步 | 99 | 83 |
| 长间隔子集发布间隔P95(ms) | 17.9568 | 17.8542 |
| 长间隔子集取帧间隔P95(ms) | 27.1470 | 26.9315 |
| 系统QPC间隔P95(ms) | 27.783 | 27.783 |

这说明本夹具中约73–76%的长间隔在发布完成→取帧间出现最大的间隔扩大；不能直接区分WGC交付与worker取帧调度，不能据此声称生产采集队列或驱动是根因。此前优先归因系统提交后的假设已收窄：下一步区分通知前等待、通知后worker调度、实际TryGetNextFrame取帧；保持像素版本关联和非阻塞/最新请求规则。

对照 Win32CaptureSample 的 SimpleCapture.cpp（https://github.com/robmikh/Win32CaptureSample/blob/master/Win32CaptureSample/SimpleCapture.cpp）：它在 FrameArrived 内调用 TryGetNextFrame 并复制；我们目前回调只唤醒worker，后者读取帧。此差异提供下一实验方向，不代表样例的回调线程/D3D上下文做法可直接用于多来源效果链路，未复制代码或改生产采集。

新增纯回归覆盖取帧前和提交后扩大、跳帧及乱序/身份拒绝。两轮原生35秒测量通过；最终源码门禁94942 exit0：workspace520通过/56忽略、Clippy、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-cadence-{observation-1,observation-2,workspace,clippy,release}.log 与 observation-{1,2}.txt。此批只有测试诊断，生产行为未改，AGENTS.md 无冲突未改；完整App/IME/双屏/资源/物理显示与20ms目标仍未完成。


### 回调提前取得帧的独立实验与撤回（2026-10-04）

仅测试实验：FreeThreaded FrameArrived 回调取得最新帧，每来源单槽替换持有，旧帧通过FrameLease归还池；worker仍唯一执行D3D比较/复制/效果与提交。记录回调取得帧后的ReadyFrame及SystemRelativeTime，按token/ticks与实际冻结输出版本精确匹配，重复时间戳拒绝；不匹配最近通知。关闭先退休回调，再撤销事件并清空单槽。此实验不证明完整App或快速跨屏关闭的帧生命周期已验收。

| 指标 | ON1 | OFF1 | ON2 | OFF2 | OFF3 | ON3 |
|---|---:|---:|---:|---:|---:|---:|
| OS对应来源计数FPS | 59.772 | 58.601 | 59.502 | 59.569 | 58.568 | 59.534 |
| 系统QPC间隔P95(ms) | 27.783 | 27.784 | 27.783 | 27.783 | 27.783 | 27.783 |
| paint发布→系统同步上界P95(ms) | 34.6932 | 35.8603 | 34.9166 | 36.2411 | 34.7350 | 34.3238 |
| 精确ReadyFrame匹配/歧义 | 1792 / 0 | 未启用 | 1784 / 0 | 未启用 | 未启用 | 1785 / 0 |
| 发布完成→回调取得帧P95(ms) | 14.2079 | 未启用 | 14.0126 | 未启用 | 未启用 | 13.3615 |
| 回调取得帧→worker处理P95(ms) | 0.9204 | 未启用 | 0.9957 | 未启用 | 未启用 | 1.0898 |

ON为提前取得帧，OFF为原实现，各35秒。初始两组显示约1ms级端到系统同步差异，因此追加第三组确认；原实现OFF3低于ON1/ON2，六轮范围重叠，系统间隔均不改善。本轮没有足够证据支持为这点差异引入生产跨线程帧持有机制，实验代码已精确撤回。此结论是收益不足以采用，不是证明收益绝对为零。

精确匹配仍提供了下一步依据：本夹具worker在已取得帧之后约1ms内处理，主要等待发生于取得帧前（包括系统交付、回调调度和TryGetNextFrame相关时间），尚不能把约14ms当WGC固有下限。后续机制实验应先区分回调进入与实际取得帧的时刻，并确认本机MinUpdateInterval能力/设置；再按证据选取适配路径，不能通过全局禁录或递归采回自身达成优化。并继续UI边界与完整资源/实机验收，避免只剩GPU诊断。

对照来源：Win32CaptureSample SimpleCapture.cpp（https://github.com/robmikh/Win32CaptureSample/blob/master/Win32CaptureSample/SimpleCapture.cpp）在FrameArrived取得帧；OBS WinRT适配与本项目的线程/上下文所有权不同（https://github.com/obsproject/obs-studio/blob/master/libobs-winrt/winrt-capture.cpp）。没有复制源码或改变生产接口。官方属性参考 https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.minupdateinterval 。

实验编译曾有内部类型可见性/未使用诊断字段警告，收窄可见性并接上匹配统计后完成测量。保存并恢复五份实验前源码（annotation_gpu、performance、arrival、capture、diagnostics），保留此前已验收的许可和相邻版本诊断。恢复后门禁85888 exit0：workspace520通过/56忽略、Clippy、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-early-dequeue-{on1,off1,on2,off2,off3,on3}.{log,txt} 和 restored-{workspace,clippy,release}.log；源码副本 target/annotation-early-dequeue-before-{0,1,2,3,4}.rs。AGENTS.md没有本批难以调和的冲突，未修改。物理显示和整轮目标仍未完成。


### 编辑参数、组件样式与布局所有权收口（2026-10-04）

生产改动：annotation_parameters.rs::updated_text_style 统一普通文字对象与活动草稿的参数合并，保留内容、旋转、连接箭头、layout_scale和未调整的分数尺寸。annotation_editor.rs::State::apply_text_style/apply_caption_style 拥有编辑原型更新、组字预览、布局缓存及输入组件同步；caption更新校验对象身份。configure供新建与两类更新共用，说明同步完整粗体/斜体/内边距；place/viewport和preview迁入编辑模块，接收显示Rect快照，不依赖Registry或CanvasLayer。协调器保留身份/参数事务/Core修改/回填/焦点路由，删除重复字体setter和原参数合并分支。非文字编辑的参数更新不额外收集显示Rect。

新增实际Slint运行时回归：100%/125%逻辑比例 × 普通文字/序号说明，保留同一实例、选区、值和UpdateComposition的中文拼音组字；调参后CommitComposition在原位置得到“A字中B”。旧caption目标拒绝，结束后的实例拒绝样式更新。使用已依赖的i-slint-core 1.18.1内部后端事件模拟，不增加依赖；此证据不等同于Windows真实输入法。共用画布渲染器导出的125%文字与说明对照已查看，文字/徽章/颜色/粗斜体可见，PNG位于 target/annotation-editor-style-{text,caption}-125.png，原始像素.rgba也保留；受控绘制图不能替代真实App截图。

中间修复：非穷尽KeyEvent按Default后字段赋值；测试badge先布局再加入Session；现有tiny-skia未启用PNG编码，改为导出像素后外部Pillow转换，不改依赖；Clippy发现生产函数位于test module之后，移动到测试模块前并重跑最终源码门禁。

最终门禁60659 exit0：workspace521通过/56忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-editor-style-final-{workspace,clippy,release}.log，受控运行时日志target/annotation-editor-style-runtime.log；第一次门禁8588因Clippy文件顺序停止，未把它当作完整通过。此批没有修改GPU调度或Core公开接口；AGENTS.md无冲突未改。整轮GPU显示目标、完整App/IME/跨屏/资源验收继续保留。


### 编辑窗口就绪后的所属关系（2026-10-04）

源码审计发现 open_text_editor_object 在原生准备之前调用 attach_tool_window 并忽略结果，Ready 显示阶段未重试。新增受控 Slint 回归先失败，确认创建组件即触发未就绪绑定（target/annotation-editor-owner-before.log）。修复把所属关系迁入 show_text_editor 的 Ready 分支，以裁剪后可见窗口原点选择画布；绑定后校准位置和 DPI，再显示并安装输入桥接，显示完成后继续校准。绑定失败取消焦点任务并结束本次编辑，不创建对象或额外撤销，不激活窗口。无画布的受控场景沿用原行为。

回归覆盖 Pending 不绑定、Ready 的 prepare→owner→bridge→activate 顺序、绑定失败清理、快速重开及旧会话/编辑请求无效；使用负坐标和1.0/1.25逻辑比例，包括原文字位置在画布外而可见窗口被裁剪进画布的情况。它验证共享文字/说明显示入口的时序，不等于真实 HWND 所属关系、物理双屏 DPI 或 Windows IME 验收；本批未生成实际 App 截图。已有编辑拥有者继续负责退休定时器和事件桥接，没有增加平行状态。

第一次门禁25499在workspace522通过/56忽略后被Clippy collapsible_if拦下，Release未执行；合并分支后最终源码门禁89642 exit0：workspace522通过/56忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和diff check通过。日志target/annotation-editor-owner-final-{workspace,clippy,release}.log，修复回归target/annotation-editor-owner-after.log。未修改GPU调度、Core公开接口或AGENTS.md，未暂存、提交或推送。

下一步继续审计操作结束/关闭交接和窗口失败后的画布失效与焦点清理；真实App/IME/跨屏/资源验收及GPU显示目标仍未完成。


### 编辑结束统一画布失效（2026-10-04）

审计上一批窗口准备失败清理时发现：finish_text_edit_registry 会退休编辑，却不请求画布刷新；外层 finish_text_edit 才刷新，因此直接调用者（原生准备Failed、Pending重试耗尽、输入桥接失败、所属绑定失败）可能保留已经显示的编辑预览/光标。受控回归先失败，确认销毁编辑后没有render pending（target/annotation-editor-retirement-before.log）。

将画布失效接入统一结束入口：只有成功take当前编辑后才queue_render，所有resolve取消/空白/失去目标及提交分支共用；外层finish_text_edit删除重复刷新。现有会话拥有的16ms渲染任务合并同帧工作，不增加第二份状态。会话整体关闭仍editor.clear并释放画布/任务，不为销毁的画布排队。审计生产editor.clear/take后，唯一clear位于Registry.close，唯一take位于统一结束入口；其他clear命中是测试。

新回归覆盖准备失败、重试耗尽和桥接失败，在已有草稿投影且前次渲染任务已消费的状态下验证结束请求新刷新、投影消失、对象/撤销不变；保留退休组件并发送迟到content_changed/done/prepare，推进600ms后验证旧输入/光标任务不建对象、不排队刷新。所属绑定失败走同一入口，复用上一批回归。此证据验证UI状态与调度，不是实际App截图或真实Windows输入法/物理显示结果。

最终门禁85747 exit0：workspace523通过/56忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和diff check通过；修复回归target/annotation-editor-retirement-after.log，完整日志target/annotation-editor-retirement-final-{workspace,clippy,release}.log。未修改GPU调度、Core公开接口、配置、依赖或AGENTS.md；未暂存、提交或推送。下一步按计划继续操作结束/关闭协议及独立GPU采集回调机制测量，完整App/IME/跨屏/资源与显示目标仍未验收。


### WGC 回调入口、通知成本与采集间隔能力（2026-10-04）

沿用上一轮归因，核实采集接口和回调通知的实际成本。当前生产创建捕获会话时，支持MinUpdateInterval的系统已请求0；运行现有原生排除测试（普通自有窗口可采集、标注视觉层及显式排除窗口不可采集），回读owned_capture_min_update_interval_100ns=Ok(0)，测试通过。日志target/annotation-capture-interval-capability.log。该值证明本机接口请求生效，不证明Windows内部没有其他交付或合成限制。

现有Arrived诊断时间取在日志锁内部，不能明确表示回调入口。增加仅测试的入口时间与通知完成端点：Subscription回调入口取Instant，经弱引用/存活校验、原子合并和wake后立即记录通知完成时间，再取得诊断锁。生产仍只有一次原子合并及必要通知，没有跨线程帧队列或D3D所有权变更。报告按入口时间排序，分开列实际唤醒次数与入口→通知完成P95；通知没有像素/帧身份，禁止用最近通知匹配帧或当作worker执行延迟。新纯回归验证入口时间保留、通知完成顺序与合并唤醒；既有4096上限/丢弃/退休回归也通过。

两轮各35秒Release高强度模糊自有源实测（CaptureAndOutput完成查询，采集timestamp查询关闭）：

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 自有源通知数 | 1800 | 1800 |
| 实际唤醒数 | 1756 | 1778 |
| 回调入口→通知完成P95(ms) | 0.010100 | 0.010400 |
| 通知入口间隔P95(ms) | 21.649 | 21.414 |
| WGC帧时间戳相邻间隔P95(ms) | 20.838 | 20.838 |
| worker取帧间隔P95(ms) | 23.193 | 22.442 |
| OS有效自有源更新FPS | 59.033 | 58.735 |
| OS同步间隔P95(ms) | 27.783 | 27.783 |
| GPU处理尝试P95(ms) | 1.658 | 1.222 |
| 发布完成→OS同步上界P95(ms) | 34.708700 | 34.431200 |
| 采集诊断丢弃 | 0 | 0 |

日志target/annotation-callback-entry-run{1,2}.{log,txt}。notify成本约10微秒，远小于观察到的帧间隔，不能作为主要优化目标。通知间隔已超过20ms目标，但它与取帧/显示间隔只是各自分布，不能相减求阶段延迟、假定通知与帧一一对应或宣布固定WGC下限。本批增加测量精度，没有证明整体性能改善，系统间隔目标仍未达到。

核对官方MinUpdateInterval接口：https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.graphicscapturesession.minupdateinterval?view=winrt-26100 （文档主要提供接口签名，本机能力以实测为准）。对照Win32CaptureSample的CreateFreeThreaded、回调TryGetNextFrame/CopyResource和Present1(1,0)：https://github.com/robmikh/Win32CaptureSample/blob/main/Win32CaptureSample/SimpleCapture.cpp 。样例由回调直接持有图形操作；本项目单worker拥有图形资源，且前轮提前取得帧实验无稳定收益，不照搬其所有权与Present参数。

最终门禁43801 exit0：workspace524通过/56忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和diff check通过。日志target/annotation-callback-entry-final-{workspace,clippy,release}.log。native能力测试和两轮实测额外通过；它们是自有窗口夹具，不是完整App、Windows IME/双屏或输入到物理显示验收。没有项目级规则冲突，AGENTS.md未改；未暂存、提交或推送。

下一步把GDI自有源发布与实际GPU窗口源的交付节奏分开验证，保持源频率/刷新率/像素版本一致，判断夹具发布路径是否影响观察；继续按同版本端点验证取帧至呈现的刷新相位，不继续以通知函数或已回读为0的接口设置作为主要瓶颈。UI关闭协议、实机/资源与物理显示验收仍保留。


### GDI 与 DXGI 源发布对照（2026-10-04）

为区分测试源发布方式对延迟的影响，新增仅测试的performance_source模块：Publisher复用既有GDI源，或创建普通HWND的D3D11/DXGI双缓冲FlipSequential源。两者发布相同1280×720八色带及像素编号，60Hz频率、144Hz输出、强度100模糊、CaptureAndOutput完成查询和既有版本/OS同步探针保持一致。DXGI源每次重新取得buffer0，上传/复制/Present(0)，资源由测试源持有；不改变生产worker、交换链或滤波。启动时检查桌面实际八色带各三处像素，正式采集继续用既有两像素版本探针。发布端点明确为API调用边界；DXGI的Present返回不代表GPU完成或屏幕已显示。新鲜度文字口径同步更正，不把提交完成称作全部像素已显示。

交替DXGI1/GDI1/DXGI2/GDI2各35秒Release实测（target/annotation-source-{dxgi1,gdi1,dxgi2,gdi2}.{log,txt}，进程9886 exit0）：

| 指标 | DXGI1 | GDI1 | DXGI2 | GDI2 |
|---|---:|---:|---:|---:|
| 发布调用FPS | 60.001 | 60.000 | 59.999 | 60.002 |
| OS有效版本FPS | 59.935 | 59.700 | 59.966 | 58.202 |
| OS同步间隔P95(ms) | 27.783 | 27.783 | 27.783 | 27.783 |
| 发布→取帧上界P95(ms) | 9.360700 | 14.565600 | 9.767000 | 14.828900 |
| 发布→OS同步上界P95(ms) | 29.576300 | 34.678100 | 30.349500 | 34.561200 |
| GPU处理尝试P95(ms) | 2.412 | 2.191 | 2.142 | 1.155 |
| 像素版本解码数 | 1800 | 1800 | 1800 | 1800 |
| 无效/丢弃探针、混合提交、无效相邻帧对、采集诊断丢弃 | 0 | 0 | 0 | 0 |

发布方式对本机夹具的等待有可重复差异，旧GDI端到OS结果不能直接代表所有GPU窗口源。两组源OS间隔均仍27.783ms，故不能把问题全部归因GDI或把换测试源当作产品优化。各列分位数不能相减求单帧阶段成本。两轮DXGI的WGC时间戳相邻间隔P95分别27.783/20.838ms，说明节奏仍有波动，不能宣布固定下限。OS同步不是物理显示；此自有HWND模拟GPU发布，未覆盖浏览器/播放器、完整App、IME、双屏DPI或输入到光子。

实现期间编译先因format参数位置、unsafe表达式括号和ClientToScreen命名空间失败，修正后原生四轮通过。首门禁1667在workspace通过后被Clippy固定切片chunks_exact_to_as_chunks拦下，Release未执行；按建议改为as_chunks_mut::<1280>()（等价固定行填充）后最终源码77850 exit0：workspace524通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release、diff check通过。忽略数增加来自新增35秒DXGI测试，该项已在交替实验显式执行两次；切片写法修正后的测试编译由Clippy all-targets覆盖，未额外宣称重跑四轮。日志target/annotation-source-final-{workspace,clippy,release}.log。生产Release无需重编该cfg(test)模块，本次增量构建0.77s；不把这一耗时当作全量构建。

继续保留GDI和DXGI两个测量源，下一步按相同像素版本审计DXGI源的取帧/提交/同步相位，并推进UI关闭协议和实际App/资源验收。未新增Core接口、配置、依赖、crate；AGENTS.md无冲突未改；未暂存、提交或推送。整轮性能目标与物理显示要求不变。


### 验收口径按用户修正（2026-10-04）

用户授权修正60Hz源／144Hz输出的刷新节奏目标：优先定位并减少第4次刷新等待，尽量不超过3次刷新（约20.833ms）；严格≤20ms不再作为必须保证的验收线，仅保留历史参考。后续同时报告刷新周期分布、超过3次刷新比例、间隔P95、有效FPS，以及真实操作到物理显示延迟；已有≥57FPS、GPU P95≤12ms、操作到物理显示P95≤50ms目标保留。3次刷新不是对所有硬件、刷新率或源路径的硬性承诺，也不是已证明下限；不能仅移动阈值把当前约4次刷新结果标成达标。源内容变化和鼠标移动对象分别验收，OS同步不能替代物理显示。历史日志和失败结果保留当时口径，当前执行以annotation-refactor-plan.md顶部为准。


### 按实际刷新次数筛选额外等待（2026-10-04）

根据用户更新的60Hz／144Hz验收口径，测试报告不再用>20ms作为需重点归因的帧对集合。CadenceFrame保留OS sync_refresh，CadencePair用checked_sub获得实际刷新计数差并拒绝重复/回退；超过3次刷新才进入long阶段分析。同版本纯回归扩展覆盖3/4次刷新以及计数回退/重复。报告额外输出有效计数样本、超过3次的数目和比例、刷新次数P95及3次周期的推算毫秒值；历史20ms比例仍保留，旧refresh_reference_target_ms=20_unchanged改为historical_reference_not_acceptance_gate。没有用毫秒四舍五入猜刷新次数，没有修改生产调度或移动验收线宣称达标。

先用上一批四轮已保存OS计数直方图复算：DXGI1/GDI1/DXGI2/GDI2超过3次比例分别9.6272%/9.7765%/8.1758%/8.6533%，即大多数更新已经在3次以内，但尾部足以使P95落到4次。该复算不关联阶段，不能说明哪一步造成尾部。

新报告三轮35秒原生实测（DXGI、GDI、DXGI复测）：

| 指标 | DXGI第1轮 | GDI | DXGI第2轮 |
|---|---:|---:|---:|
| 有效刷新计数间隔 | 1798 | 1784 | 1774 |
| 超过3次刷新 | 151 | 126 | 133 |
| 超过3次比例 | 8.3982% | 7.0628% | 7.4972% |
| 刷新次数P95 | 4 | 4 | 4 |
| OS间隔P95(ms) | 27.783 | 27.783 | 27.783 |
| 有效OS版本FPS | 59.969 | 59.501 | 59.168 |
| 超限同像素版本帧对 | 151 | 125 | 133 |
| 超限且跳过源编号帧对 | 0 | 8 | 22 |
| 最大扩大：发布→取帧 | 34 | 66 | 31 |
| 最大扩大：取帧→提交 | 21 | 36 | 55 |
| 最大扩大：提交→OS同步 | 96 | 21 | 33 |

超过3次的计数分母为有效OS计数间隔；阶段分母只含能关联完整像素版本/发布端点的帧对，不能混用。GDI的126对与125对不同来自覆盖范围不同；并非用missing阶段补猜。实际推算3次刷新20.8374ms（计数时钟约143.97Hz），不是硬编码20ms。最大扩大只是同一相邻帧对的带符号间隔变化，不是GPU执行时间或单阶段绝对延迟，三类数量可以小于全部帧对（没有正扩大时不强行归因）。

DXGI首轮96/151最大扩大在提交→同步，复测33/133且取帧→提交55/133，不能据首轮确定合成是唯一瓶颈。复测还包含22对跳帧和更大的发布间隔，下一步须将连续源帧、跳帧和发布抖动分开，重复关联同版本阶段/刷新相位，不能继续从总体分位数相减或直接修改Present。用户关心的鼠标移动对象链路仍需独立测量，不能由源内容变化替代；OS同步仍不等于物理显示。

最终源码门禁3268 exit0：workspace524通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和diff check通过；原生DXGI/GDI回归额外通过，门禁后未改源码，65002 DXGI复测也exit0。日志target/annotation-refresh-cycles-{dxgi,gdi,dxgi2}.{log,txt}，target/annotation-refresh-cycles-final-{workspace,clippy,release}.log及annotation-refresh-cycle-test.log。本批只有测试分析变更，Release增量构建0.69s，不称全量构建。未新增Core接口/依赖/配置，AGENTS.md无冲突未改；未暂存、提交或推送。完整交互/实机/资源/物理显示要求继续保留。


### 父窗口关闭前停用输入（2026-10-04）

操作与关闭协议审计发现：单独关闭二级参数工具条或水印输入窗口时，先关闭子菜单，再由Slot.clear停用父窗口输入。Menu::Drop移除原生关闭监控时可能同步回送父窗口鼠标事件；此时Registry仍被关闭路径可变借用，父窗口仍为active，pointer_panel/current水印路由进入Registry造成RefCell重复借用。整个会话关闭原本已提前停用所有输入，问题位于两个单独关闭入口。

新增受控Slint回归owner_close_rejects_reentrant_pointer_input_from_child_menu_teardown：让真实子菜单的原生dismissal清理回调同步调用父窗口pointer sink，并在父窗口队列中放入待执行任务。参数工具条未修复时在panel_current处panic；保留参数修复、单独撤去水印修复后，水印current检查同样panic，随后测试线程销毁再次panic而abort。日志分别为target/annotation-owner-retirement-before.log、annotation-watermark-owner-retirement-before.log，二者均为负向复现实验，不是通过结果。

两个入口现在先Slot.retire_input，再结算焦点/参数、销毁子菜单和父窗口；保持原有Slot.clear身份更新与窗口销毁，不新增Core接口或平台机制。修复后同一测试覆盖两种父窗口、同步回调拒绝、待执行任务清理和关闭重开后旧sink失效；新实例仍active、三级菜单无残留。相关annotation回归139通过/3忽略，日志target/annotation-owner-retirement-regression.log；单例修复后通过日志annotation-owner-retirement-after.log。这是受控事件清理回归，不等同于实际Windows原生重入或App截图验收。

本批不修改GPU调度，不宣称27.783ms下降。刷新次数、不同源路径和真实操作到物理显示验收继续执行顶部现行口径；AGENTS.md未发现难以调和冲突，未改；不暂存、提交或推送。最终源码门禁64496 exit0：workspace525通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和git diff --check通过。日志target/annotation-owner-retirement-final-{workspace,clippy,release}.log；未将受控软件窗口测试称为实机截图验收。


### 三级菜单独立关闭的原生输入交接（2026-10-04）

上一批父窗口退休保护不能用于独立关闭三级菜单：二级参数工具条或水印输入窗口仍需保持可用。复用上一批原生清理回调夹具，抽取共同测试装配exercise_menu_owner_teardown，新增child_menu_close_rejects_cleanup_input_without_retiring_its_owner。未修复时只关闭三级菜单也在panel_current的Registry借用处panic，target/annotation-child-retirement-before.log记录负向复现（66280 exit1）。

InputWork增加可嵌套的RAII PointerPause。pointer_sink同时检查active和暂停深度；暂停仅抑制当前同步清理范围内的原生鼠标回送，不停止组件FIFO队列、不更改实例身份、不激活窗口。close_from结算参数后，在drop(menu)原生监控/窗口清理期间持有对应父窗口的暂停作用域，退出后恢复鼠标转发，再执行原有输入控件和焦点交接。真正退休仍由active控制，暂停作用域释放不能重新激活已退休实例。模块职责仍为UI瞬时输入生命周期，没有向Core或Platform增加接口，也没有全局丢弃普通输入或使用try_borrow掩盖任意重入。

受控集成回归覆盖参数工具条、水印输入窗口、父窗口关闭与菜单独立关闭两种路径：清理回调确实执行但不进入owner sink；独立关闭后同一父窗口实例仍active，原有排队任务执行，新的鼠标事件可到达，菜单无残留；真正父窗口关闭后队列丢弃，重开身份变化，旧sink不能操作新窗口。InputWork回归额外验证嵌套作用域、队列保留以及暂停期间退休后不可恢复。annotation相关回归141通过/3忽略，target/annotation-child-retirement-regression.log。这些是软件窗口和注入的原生清理回调证据，未声称实际Windows/App截图、IME或双屏已验证。

本批不修改GPU调度与焦点授权规则；刷新周期现行目标及真实物理显示、App/资源验收继续保留。最终源码门禁59911 exit0：workspace527通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release、git diff --check通过；日志target/annotation-child-retirement-final-{workspace,clippy,release,diff}.log。AGENTS.md无难以调和冲突未改；未暂存、提交或推送。


### 区分发布正常、发布超预算与跳源编号（2026-10-04）

本批继续>3次刷新等待归因，仅修改Platform cfg(test)诊断与报告。CadencePair按实际同像素版本的skipped_paints和publication API完成间隔分三组：连续且发布间隔≤预算、连续但发布间隔>预算、显示时跳过源编号。预算为有效OS计数/QPC推算刷新周期中位数的3倍，不硬编码144Hz或20ms；无效预算返回未分类，各组使用自身全部有效帧对作分母，空组比例/延迟为unavailable。largest_growth抽为共用方法，整体和分组分析使用相同判定。纯回归覆盖阈值边界、跳编号优先、无正扩大、无效/NaN/无限预算；3个cadence相关测试通过，target/annotation-cadence-source-regression.log。

三轮相同35秒场景按DXGI/GDI/DXGI运行（76758 exit0），source_fps59.998/60.002/59.999，源间隔P9517.840/17.873/17.783ms。像素探针invalid/dropped均0，cadence非法帧对/未分类均0，推算3次刷新预算均20.8374ms。

| 指标 | DXGI1 | GDI | DXGI2 |
|---|---:|---:|---:|
| 有效OS计数间隔 | 1791 | 1789 | 1798 |
| 全部>3次数 | 180 | 184 | 148 |
| 全部>3比例 | 10.0503% | 10.2851% | 8.2314% |
| OS有效版本FPS | 59.731 | 59.668 | 59.966 |
| OS间隔P95(ms) | 27.783 | 27.783 | 27.783 |
| 连续且发布正常帧对 | 1783 | 1779 | 1797 |
| 该组>3次数/比例 | 173/9.7027% | 179/10.0618% | 148/8.2359% |
| 该组长间隔最大扩大：发布→取帧 | 60 | 81 | 43 |
| 该组长间隔最大扩大：取帧→提交 | 16 | 59 | 14 |
| 该组长间隔最大扩大：提交→OS同步 | 97 | 39 | 91 |
| 连续但发布超预算帧对/其中>3次数 | 2/1 | 0/0 | 0/0 |
| 跳源编号帧对/其中>3次数 | 6/6 | 9/5 | 0/0 |

分组的同像素版本关联帧对总数1791/1788/1797，与全部有效OS计数间隔1791/1789/1798的覆盖范围不同；GDI少一对和DXGI2少一对不能补猜或混用分母。跳编号表示显示关联的源编号存在缺口，不直接断言是哪一步丢帧。发布完成是GDI/DXGI调用返回，不等于源GPU完成；阶段最大扩大仍是相邻版本间隔的带符号变化，不是单帧阶段执行时间，阶段分位数不能相加。

结果收紧下一步：排除显示跳编号和发布已超预算后，尾部仍存在。DXGI正常发布连续组中提交→OS同步最大扩大97/173及91/148，两个本批重复都占多数；GDI相同组最大扩大偏发布→取帧81/179，不能用GDI结论代替DXGI。与上一批DXGI复测有变化，本批仍不宣称DComp为唯一瓶颈，不以阶段分位数相减确认截止时间；下一GPU实验须关联实际提交相位/合成统计与同版本同步，保留分源路径和连续发布条件。鼠标操作到物理显示仍没有被本源变化夹具证明。

针对呈现等待，复查了[Microsoft DirectComposition架构](https://learn.microsoft.com/en-us/windows/win32/directcomp/architecture-and-components)：文档描述合成器异步取待处理批次、跨过批次收取时刻会等待下一合成帧。这给出“提交相位可能增加刷新等待”的机制假设，不是本机已证实原因，也不能据旧版本文档保证Windows当前截止时刻。检查[官方DXGI低延迟指南](https://learn.microsoft.com/en-us/windows/uwp/gaming/reduce-latency-with-dxgi-1-3-swap-chains)和[微软LowLatencyInput开源DeviceResources](https://github.com/microsoft/Windows-universal-samples/blob/main/Samples/LowLatencyInput/cpp/Common/DeviceResources.cpp)：指南说明waitable交换链及每交换链低排队设置、先等待再取输入/渲染；源码WaitOnSwapChain使用等待，Present用(1,0)。本仓surface.rs已经waitable且SetMaximumFrameLatency(1)，所以不是缺少这两个选项。不能将UWP阻塞循环直接搬入本仓最新帧MTA worker，尤其Present(1,0)旧实验没有稳定收益；需要测同版本相位后才决定实验，不凭参考实现重写或增加固定等待。

最终源码门禁22250 exit0：workspace528通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和git diff --check通过。日志target/annotation-cadence-source-{dxgi1,gdi,dxgi2}.{log,txt}、annotation-cadence-source-final-{workspace,clippy,release,diff}.log。Release0.60s为测试专用改动后的增量构建，不称全量构建。本批未改生产调度、不宣称性能收益；AGENTS.md兼容未改，未暂存/提交/推送。完整App、IME、跨屏、资源和真实物理显示验收仍保留。


### 实际DComposition Commit窗口与OS同步（2026-10-04）

呈现相位审计先发现测量边界：旧Presentation.at在present_surfaces完成及worker记账后记录，包含Commit后的SetWindowPos等CPU工作，不能当作原生Commit调用本身。旧指标保留供历史对照，不重写旧结果。本批在Surface::present的cfg(test)路径记录实际DComposition::Commit调用前/后Instant，失败/重试开始清空旧样本；源提交记录仅在一个Surface和一个交换链计数时附带该窗口，不将多屏部分查询成功强行关联到第一Surface。纯映射以同像素版本对应的OS时钟上下界给出Commit→OS同步区间，并单独报告Commit调用宽度与调用结束→旧accepted标记尾部。

whole_refreshes使用同版本同步延迟上下界和实测刷新周期；区间跨整数边界时保留ambiguous，不用四舍五入猜等待次数。该桶表示相隔多少个完整刷新周期，不代表错过多少次DWM截止时间。native_commit_bounds_exclude_accepted_tail_and_preserve_refresh_ambiguity覆盖尾部分离、时序反转、同步不确定范围、无效时钟/周期及边界模糊，相关测试通过，target/annotation-native-commit-regression.log。

12162三轮35秒DXGI/GDI/DXGI原生夹具exit0：

| 指标 | DXGI1 | GDI | DXGI2 |
|---|---:|---:|---:|
| 实际Commit有效样本 | 1797 | 1791 | 1796 |
| Commit缺失/无效 | 0/0 | 0/0 | 0/0 |
| 调用P95(ms) | 0.0118 | 0.0120 | 0.0122 |
| 调用结束→accepted尾部P95(ms) | 0.1155 | 0.1166 | 0.1112 |
| Commit→OS同步下界P95(ms) | 18.8913 | 19.9638 | 19.4969 |
| Commit→OS同步上界P95(ms) | 18.9010 | 19.9693 | 19.5032 |
| 全部完整周期桶1/2/3/模糊 | 437/1350/10/0 | 182/1590/14/5 | 366/1407/21/2 |
| 连续发布正常且>3次刷新样本 | 170 | 146 | 168 |
| 该组Commit→OS同步上界P95(ms) | 20.7810 | 20.8313 | 20.8764 |
| 该组完整周期桶1/2/3/模糊 | 2/161/7/0 | 1/138/7/0 | 0/158/10/0 |
| OS有效版本FPS | 59.933 | 59.737 | 59.900 |
| OS间隔P95(ms) | 27.783 | 27.783 | 27.783 |
| 全部>3次刷新比例 | 9.5159% | 8.4869% | 9.4655% |

CPU尾部约0.11ms不能解释约一个额外刷新周期；原生Commit调用本身也很短。没有用独立P95相减估算单帧差值，上下界计算属于同一匹配像素版本。这支持继续检查提交后的呈现机制，但仍未证明具体合成批次截止时间，更不是物理显示延迟或生产性能收益。三轮之后收紧诊断的单Surface条件，单Surface夹具语义不变；最终门禁与后续实测记录下方补充。

机制核查：[GetFrameStatistics](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-getframestatistics)和[DCOMPOSITION_FRAME_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/dcomptypes/ns-dcomptypes-dcomposition_frame_statistics)提供合成时间、频率和下一帧估计，官方明确估计值可能因无工作或合成成本变化而改变；nextEstimatedFrameTime不能当作已测批次收取截止时间，本批没有据此增加等待。另找到[微软MSDN原生透明交换链示例](https://learn.microsoft.com/en-us/archive/msdn-magazine/2014/june/windows-with-c-high-performance-window-layering-using-the-windows-composition-engine)：固定视觉树只需初始化时Commit，SwapChain Present与视觉树变更不强制同步。本仓Surface的SetContent/SetRoot也在初始化，下一单因素实验可验证省去逐帧空Commit是否仍正确显示且减少>3次尾部；初始化Commit保留。参考是适用机制与样例，不证明本机性能收益；须实测层序/动态像素/重建，收益不稳定则按实验规则撤回。

本批仅测试诊断，Core接口/配置/依赖及生产调度未改；AGENTS.md无冲突未改，不暂存/提交/推送。实际App、IME、双屏、完整资源和物理显示验收仍缺。

最终源码门禁17589 exit0：workspace529通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（1m10s）和git diff --check通过。日志target/annotation-native-commit-final-{workspace,clippy,release,diff}.log。门禁后源码未改，56347最终单Surface版本DXGI复测exit0：1758个Commit样本，缺失/无效0/0，调用P950.0118ms、accepted尾部0.1055ms，Commit→OS同步上界17.9497ms；正常发布连续长间隔组208样本，该上界20.5044ms。OS间隔P9527.784ms、有效FPS58.634、>3比例13.9932%，相比前轮尾部波动更大，进一步禁止把测量改进当性能收益。日志target/annotation-native-commit-{dxgi1,gdi,dxgi2,dxgi-final}.{log,txt}；三轮数据在收紧单Surface条件前采集，最终复测验证收紧后的实际源码，不隐瞒版本边界。


### 撤回省去逐帧空Commit实验（2026-10-04）

依据上一批微软固定视觉树示例，执行一次真实单因素实验：保留Surface初始化的SetContent/SetRoot/Commit，在present中仅省去逐帧DComposition Commit及与该调用绑定的测试计时，Present(0,DO_NOT_WAIT)、窗口位置/层序、捕获、GPU效果、比较、完成事件及诊断探针保持相同。实验版本的native_commit_missing正好表示没有该调用，不能填一个零长度假调用；仍保留原accepted-frame提交/OS计数/同像素发布关联。实验编译出现保有生命周期的composition字段未读取警告，仅属于实验版本；最终原版没有该警告。

96494以基线/实验交替三组，每次完整35秒原生DXGI源大模糊夹具（5秒暖机+30秒采样），六轮全部exit0。source_fps分别60.001/60.000/60.002/60.001/60.002/60.001，像素探针invalid/dropped均0。不是以单次历史日志代替当次基线，也没有在计时期间运行workspace构建或其他GPU测试。

| 指标 | 基线1 | 实验1 | 基线2 | 实验2 | 基线3 | 实验3 |
|---|---:|---:|---:|---:|---:|---:|
| OS有效版本FPS | 59.368 | 58.867 | 59.802 | 59.901 | 58.502 | 59.101 |
| OS间隔P95(ms) | 27.783 | 27.783 | 27.783 | 27.784 | 27.784 | 27.784 |
| 有效刷新计数间隔 | 1780 | 1765 | 1793 | 1796 | 1754 | 1772 |
| >3次数 | 241 | 232 | 150 | 238 | 280 | 269 |
| >3比例 | 13.5393% | 13.1445% | 8.3659% | 13.2517% | 15.9635% | 15.1806% |
| 连续发布正常组>3比例 | 12.6633% | 11.4962% | 8.1655% | 13.1623% | 13.8596% | 14.0321% |
| 发布→OS同步上界P95(ms) | 30.1415 | 32.5022 | 28.8823 | 30.3782 | 30.2078 | 30.9988 |
| 实际Commit缺失样本 | 0 | 1765 | 0 | 1796 | 0 | 1772 |

去掉调用仍可关联持续更新的源版本与OS呈现，但没有把这一点当作完整动态效果/透明层/设备重建的像素正确性验收。性能已不满足保留实验的条件，未扩大无益实验的功能测试：>3比例和FPS没有重复一致改善，发布→OS同步上界三组均更差，OS间隔仍约4次刷新。它可能在特定节奏下改变提交/唤醒相位，现有证据不能进一步断言；也不能据此推论所有固定视觉树都必须逐帧Commit。仅结论为本仓此次省去调用没有稳定整体收益，按计划撤回。

保存当次唯一变更文件的字节快照并校验SHA256：基线surface.rs为1711c4f4aae422beeb42ebe584d480e305d78450f6a3ee4974d350b80b462600，实验为72caef9632303138739799bf7f13f2f514631b99fe403f79a9fa83f878556859；每次切换只允许这两个已知哈希，否则拒绝覆盖。六轮完成后恢复当次原始字节，最终哈希与基线完全相同，没有用Git重置用户改动。记录target/annotation-noop-commit-{baseline,experimental}{1,2,3}.{log,txt}、annotation-noop-commit-sources.json和本次target下原始/实验快照。

下一GPU步骤应读取实际输出Surface的合成统计并与其提交窗口、同像素OS同步关联，先确认合成频率及估计时间是否适用；nextEstimatedFrameTime是官方估计，不能作为已测批次收取截止时间。暂不继续试空Commit或据GPU算法重写预设收益。其他标注交接、场景失效和完整App/资源/IME/双屏/物理显示验收继续保留。本批未新增Core接口/配置/依赖，AGENTS.md无冲突未改，未暂存/提交/推送。恢复后最终门禁87899 exit0：workspace529通过/57忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（1m11s）和git diff --check通过；日志target/annotation-noop-commit-final-{workspace,clippy,release,diff}.log。门禁结束再次核对Surface SHA256与基线一致；此次无永久业务源码变更，未宣称性能目标达成。


### 同设备合成统计探针与未来lastFrameTime（2026-10-04）

本批新增Platform cfg(test)模块composition_timing，读取实际Surface同一IDCompositionDevice的GetFrameStatistics，保存调用窗口、QPC校准和原始统计，关联单Surface/单交换链计数、同像素源版本及其OS同步。Mailbox/Renderer/Surface的composition_probe仅测试可见，默认关闭；现有所有基线入口保持关闭，新增ignored原生入口gpu_live_capture_paint_latency_with_composition_clock显式开启。生产代码不读取统计，不用估计调度或增加等待；Core接口/配置/依赖未改。

第一轮探针错误地假设lastFrameTime≤currentTime，25585的on/off/on数据中1742/0/1741个开启样本被该假设挡住，不能当作可用相位数据。补最多3个原始无效例子（热路径只存复制字段，格式化在事后报告）：34738原生回归缺失0、无效1738，首例lastFrameTime=2318365805486,currentTime=2318365716440,timeFrequency=10000000,nextEstimatedFrameTime=2318365874944,anchor_qpc=2318365716452,display_qpc=2318365874950，API/校准/Commit窗口顺序有效。lastFrameTime比currentTime晚8.9046ms，而下一估计仅比实际匹配OS同步早6个QPC tick。此证据推翻本批“last必须在过去”的校验假设，不是系统API失败。

重新查看[官方统计结构](https://learn.microsoft.com/en-us/windows/win32/api/dcomptypes/ns-dcomptypes-dcomposition_frame_statistics)与[GetFrameStatistics](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-getframestatistics)：结构定义合成时间/速率，下一帧字段是显示时间估计，并且可能因工作/成本改变；未给出本批假设的past-time不变量。删除该额外限制，将currentTime-lastFrameTime保留有符号差值，不再命名为非负age；频率、计数、率分子/分母、正数原始字段、校准/查询/Commit顺序仍校验，未来/过期估计不被钳成零或算作截止时间失败。纯回归覆盖未来last、负估计lead、OS同步早于估计、时钟频率不匹配及无效率；修复后通过，target/annotation-composition-clock-regression.log。

63611修复后35秒ON/OFF/ON对照全部exit0：

| 指标 | ON1 | OFF | ON2 |
|---|---:|---:|---:|
| 合成统计有效样本 | 1764 | 不读取 | 1736 |
| 缺失/无效 | 0/0 | 0/0 | 0/0 |
| 报告合成周期P95(ms) | 6.9458 | 不可用 | 6.9458 |
| GetStats+QPC调用窗口P95(ms) | 0.0050 | 不读取 | 0.0049 |
| 下一估计相对Commit结束lead上界P95(ms) | 17.5773 | 不可用 | 17.4267 |
| 同版本OS同步减下一估计P95(ms) | 0.0006 | 不可用 | 0.0006 |
| 连续发布正常且>3次刷新统计样本 | 240 | 不读取 | 213 |
| 该组估计lead上界P95(ms) | 20.3388 | 不可用 | 20.4132 |
| 该组OS同步减估计P95(ms) | 0.0026 | 不可用 | 0.0009 |
| OS有效版本FPS | 58.833 | 59.834 | 57.901 |
| OS间隔P95(ms) | 27.784 | 27.783 | 27.784 |
| >3次刷新比例 | 15.4762% | 11.9844% | 15.6106% |
| 发布→OS同步上界P95(ms) | 31.0925 | 29.6307 | 30.4300 |

率的精确有理数直方图ON1为(10000000,69458):1755,(10000000,69459):9；ON2为1715/21，约143.974Hz，和该输出实测刷新周期一致。没有误用源60Hz作为合成60Hz，也未使用固定144推算。表中OS同步减估计为有符号误差，其P95接近零不能排除较大的负误差，不能据此断定长间隔样本普遍贴合预测。尚未保存并比较相邻版本的绝对估计时间，本批没有定位第四次刷新出现的具体时刻，也没有实测DWM收批次截止点。

开启两轮FPS低于中间关闭轮且>3比例更高，虽单次读取P95仅约5μs，对照没有证明零扰动，不能用ON吞吐量代替未仪表化生产值；探针保留为显式选用工具，下一关联须继续校准开销，必要时稀疏读取。前一套错误校验的数据/原始复测保留为历史实验，不与修复后结果混用。日志target/annotation-composition-clock-{on1,off,on2,raw,valid-on1,valid-off,valid-on2}.{log,txt}。

补充绝对误差指标后，40206原生复测有效样本1761、缺失/无效0/0；整体绝对误差P95为0.0008ms，但连续发布正常且>3次刷新组219个样本的绝对误差P95为6.946ms，约一个输出刷新周期。该结果否定前述仅根据有符号P95推断长间隔组普遍贴合估计的强结论。当前查询在SwapChain Present之后、DComposition Commit之前；下一步检查正负误差分布、Present前后估计与相邻绝对估计的关联，暂不把查询时序或预测偏差认定为第四次刷新原因。该轮OS有效版本58.734FPS、>3次比例14.4804%，不代表物理显示或生产性能提升。日志target/annotation-composition-clock-absolute.{log,txt}。

最终源码门禁40206 exit0：workspace530通过/58忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（增量0.62s）和git diff --check通过；日志target/annotation-composition-clock-final-{workspace,clippy,release,diff}.log。无生产性能改善声明，AGENTS.md无冲突未改，未暂存/提交/推送。完整App/交接/场景失效/资源/中文IME/双屏/物理显示验收仍未完成。

### 合成估计有符号周期分布（2026-10-04）

上一批绝对误差揭示长间隔组可能偏差一个刷新周期，但没有确定方向。本批在仅测试Sample中按自身实测合成周期对(OS同步−估计)四舍五入，保留正负周期直方图及无效计数。此分组不是OS实际刷新次数，不表示已测错过收批次截止时间；半周期内误差归零，不代表精确相等。纯测试覆盖实际同步早/晚一个周期、近零误差、无效周期。无新增生产查询、调度、Core接口或依赖。

64174及62407两次35秒原生夹具均成功，缺失/无效统计0/0：

| 指标 | 第一轮 | 第二轮 |
|---|---:|---:|
| 全部估计样本 | 1764 | 1766 |
| 全部误差周期直方图 | {0:1756,+1:8} | {0:1743,+1:23} |
| 连续正常发布且>3次刷新组样本 | 242 | 214 |
| 该组误差周期直方图 | {0:236,+1:6} | {0:197,+1:17} |
| 该组绝对误差P95(ms) | 0.0012 | 6.9458 |
| OS有效版本FPS | 58.837 | 58.902 |
| OS间隔P95(ms) | 27.784 | 27.784 |
| 实际刷新>3比例 | 15.6463% | 13.8732% |

两轮未观察到负一周期误差；本批发现的是少数实际同步晚于预测约一周期，长组比例6/242与17/214决定绝对误差P95是否跨到一个周期。上一批“有符号P95可能隐藏负误差”是统计上的可能性，本次方向证据不支持将其当作实际原因。两轮有符号分布与上一批单一绝对P95不能混成同批数据；尚未证明预测偏差导致第四次刷新，也没有生产性能改善。

下一步核对Present前/后估计的同频绝对值与相邻源版本，同像素关联发布/取帧/Present/Commit/OS同步，继续校准仅测试读取的扰动；不把下一估计当作硬截止时间。日志target/annotation-composition-clock-direction{,2}.{log,txt}。完整App/IME/双屏/资源/物理显示验收仍未完成。AGENTS.md兼容未改；未暂存、提交或推送。

最终门禁62407 exit0：workspace531通过/58忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（增量0.63s）和git diff --check通过；日志target/annotation-composition-direction-final-{workspace,clippy,release,diff}.log。

### Present前后同设备估计配对（2026-10-04）

上一批确定一周期偏差方向，本批新增仅测试的Present前查询，保留成功Present后/Commit前查询，两者随同一次NativeCommit记录，再按同像素源版本/交换链计数关联OS同步。失败Present的前查询不成为已显示样本。Present CPU计时仍在前查询结束后开始，不将查询混入API耗时。比较使用同频QPC差值及有序查询窗口，无效时不猜测；纯回归覆盖正负预测变化、频率不一致及顺序颠倒。全部查询默认关闭，无生产调度变化。

67055、88212两轮35秒原生夹具通过，统计缺失/无效0/0；按各样本实测周期取整的差值不是实际刷新计数：

| 指标 | 第一轮 | 第二轮 |
|---|---:|---:|
| 前后完整配对样本 | 1740 | 1794 |
| 前后未配对样本 | 0 | 0 |
| 预测前进周期直方图 | {0:1728,+1:12} | {0:1788,+1:6} |
| Present前实际同步误差 | {0:1691,+1:49} | {0:1754,+1:40} |
| Present后实际同步误差 | {0:1703,+1:37} | {0:1760,+1:34} |
| 正常发布长间隔组样本 | 190 | 217 |
| 该组预测前进周期 | {0:184,+1:6} | {0:213,+1:4} |
| 该组Present后误差 | {0:172,+1:18} | {0:199,+1:18} |
| 该组后查询绝对误差P95(ms) | 6.9460 | 6.9459 |
| OS有效版本FPS | 58.032 | 59.837 |
| 实际刷新>3比例 | 14.1379% | 12.2631% |

前后预测偶尔前进一个周期，但大部分不变；两轮长间隔组仍各有18个后查询样本的实际同步晚于估计约一周期。不能用前后查询跨周期解释全部长间隔，也不能从这些差值推断API改变预测或错过DWM截止点。第一轮前/后查询窗口P95约0.0064/0.0035ms；诊断虽很短仍可能影响相位，未做无探针性能收益宣称。第一轮OS间隔P95仍27.783ms。日志target/annotation-composition-present-pair{,2}.{log,txt}。

下一步关联相邻已显示版本的绝对预测间隔与发布/取帧/Present/Commit/OS同步，区分已反映于预测的长间隔与额外偏差；校准探针扰动，并独立推进交互/场景失效与完整资源验收。AGENTS.md兼容未改；未暂存、提交或推送。真实App/IME/双屏/物理显示验收尚未完成。

最终门禁88212 exit0：workspace532通过/58忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release及git diff --check通过。日志target/annotation-composition-present-final-{workspace,clippy,release,diff}.log。

### 相邻已显示版本的预测间隔（2026-10-04）

本批仅测试Sample保存原始预测/OS同步QPC及频率，按同交换链、递增呈现计数和相邻已识别像素版本关联。以i128先求差再转换ms，保留预测不变/倒退，频率不一致或实际同步不递增时无效；不是将两组P95相减。纯测试覆盖长运行时小间隔精度、正扩大、预测不变/倒退、异频及重复同步。新报告分别关联Present前和后查询，并保持正常连续发布长间隔组；缺失不参与分类，无生产链路变化。

1631与86205两次35秒原生夹具通过，两组前/后配对间隔均缺失/无效0/0：

| 指标 | 第一轮 | 第二轮 |
|---|---:|---:|
| 相邻已显示版本对 | 1768 | 1786 |
| 后查询预测间隔P95(ms) | 27.7832 | 27.7832 |
| 同版本实际同步间隔P95(ms) | 27.7833 | 27.7834 |
| 后查询扩大周期直方图 | {-1:21,0:1727,+1:19,+2:1} | {-1:25,0:1736,+1:25} |
| 正常连续发布长间隔组版本对 | 187 | 187 |
| 该组后查询扩大周期 | {0:171,+1:16} | {0:168,+1:19} |
| 该组前查询扩大周期 | {0:169,+1:18} | {0:166,+1:21} |
| 该组后查询预测间隔P95(ms) | 34.7286 | 34.7291 |
| 该组实际同步间隔P95(ms) | 34.7286 | 34.7292 |
| 该组同对扩大P95(ms) | 6.9460 | 6.9461 |
| OS有效版本FPS | 59.002 | 59.601 |
| 实际刷新>3比例 | 12.0972% | 11.0800% |

“扩大周期”是同对实际间隔−预测间隔除以各样本合成周期取整，不是实际刷新次数。正常组约91.4%/89.8%的后查询间隔差处于半周期内，约8.6%/10.2%又扩大一周期；两轮前查询也大部分无额外扩大。说明大部分长间隔已反映于提交前预测，仍有额外偏差。不能由此确定预测形成机制、DWM收批次截止时间或排除提交后的所有等待。长组P95约34.73ms包含超过4周期的尾部，不能用它替代全部样本约27.78ms的P95。

同组发布/取帧/accepted提交间隔P95第一轮17.9211/24.4748/26.9489ms、第二轮17.7643/24.2784/27.4683ms；最大扩大阶段计数第一轮45/82/60、第二轮36/60/91（发布→取帧/取帧→提交/提交→同步），主导阶段并不稳定，不能只优化某一个阶段就宣称解决。日志target/annotation-prediction-intervals{1,2}.{log,txt}。

下一步应按是否额外扩大分组连接实际Present前的worker/许可/计算阶段，做单因素实验且保留无探针对照；同时推进场景失效CPU工作和操作交接。此次无生产性能改善，AGENTS.md兼容未改；未暂存、提交或推送。完整App/IME/双屏/资源/物理显示验收保留。

最终门禁86205 exit0：workspace533通过/58忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release及git diff --check通过；日志target/annotation-prediction-final-{workspace,clippy,release,diff}.log。

### 光标闪烁交互帧复用（2026-10-04）

本批推进CPU场景失效。原interaction CacheEntry只保留当前交互帧；visible翻转导致重绘交互层，显示光标时draw_caret重新调用共用文字排版。先增加回归：39644在旧实现失败，第三次切回显示状态生成revision3而非复用revision1。随后仅在annotation_scene_interaction中为每个视口保留当前帧及至多一个相反光标可见态的局部像素帧。

复用要求选中/悬停对象、编辑正文、插入/选区、组字范围、基线参考、scale及栅格几何完全相同，仅caret.visible不同；首次翻转绘制，之后交换两份不可变RasterLayer，不再重绘/排版。任何其他变化丢弃旧对。重用旧revision后，新内容从两份帧最高revision继续递增，防止同RasterKey被赋予不同像素。平台按完整key判等，旧revision不是过期回调；会话/场景stamp机制未变。

95892及94995相关回归通过；1765最终场景回归11通过/1忽略，包含普通文字与序号、局部跨视口、旋转、大字号、负坐标、100%/125% DPI、文档/交互隔离及删除缓存。新增回归覆盖连续12次翻转像素Arc/key复用、切回旧key后改插入点的revision唯一性、颜色/字体/字号/内容/IME/基线变化，逐像素对照原软件渲染（最大差≤1）；Weak证明插入位置变化丢弃旧对、清理释放两份交互像素。移除视口及clear均清理CacheEntry，不新增timer/组件/回调/Core接口/配置/依赖。

88284 Release局部CPU夹具完成：每组120次显示/隐藏交替，基线在每次翻转前只移除alternate，保留当前状态/栅格以重现原分支；新路径保留两份帧。字体及两种可见态先预热，以下不是完整render_canvases、操作→显示或GPU延迟：

| 文本字符数 | 逻辑字号 | DPI | 原闪烁路径P95(ms) | 复用P95(ms) | 当前+另一态交互像素(bytes) |
|---|---:|---:|---:|---:|---:|
| 24 | 32 | 125% | 1.0259 | 0.0008 | 513024 |
| 240 | 32 | 125% | 11.5448 | 0.0025 | 1204224 |
| 24 | 240 | 125% | 5.9822 | 0.0016 | 4261824 |

缓存代价为每视口至多一张额外交互帧，随视口/内容边界增大；并非无额外内存或整屏渲染变快。CPU夹具Weak确认clear后双方释放。Native Renderer仍按当前场景保留纹理，下一可见态可能需要上传；不宣称消除GPU上传、解决第四次刷新或生产FPS改善。

渲染产物target/annotation-caret-cache-{true,false}.rgba以1280×720预乘RGBA输出；离线合成白底生成同名PNG，已通过view_image查看：正文、选区、四角按钮一致，仅插入光标显隐不同。这是受控渲染图，不是实际App/原生IME/双屏交互截图。日志target/annotation-caret-cache-cpu.{log,txt}、annotation-blink-cache-regression.log、annotation-caret-cache-scenes.log。初次测试编译发现tiny-skia未启用save_png，改用已有std写原始帧及离线转换，未调整依赖或feature。

GPU时序探针保持默认关闭和已有结果；下一步审计未变化场景的早期编译与参数/关闭交接，完整App/IME/双屏/资源/物理显示验收继续保留。AGENTS.md与当前实现兼容未改；未暂存、提交或推送。

最终门禁36006 exit0：workspace534通过/59忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release及git diff --check通过；日志target/annotation-caret-cache-final-{workspace,clippy,release,diff}.log。

### 相同内容提交去重与静态设备健康（2026-10-04）

本批继续CPU场景失效，确认SceneCompiler已有普通对象/几何/交互缓存，但render_canvases仍每次组装新scene并提交新stamp。未提前跳过编译：先在Presenter的提交边界抑制已成功接受的相同内容，避免不必要的worker唤醒和请求编号更换。67662新增回归在旧实现失败，重复请求的port.submit调用为2而非1。

新增UI内部annotation_scene_content纯比较：保留会话、窗口token、视口/DPI、层顺序/类型、效果对象全部字段、覆盖/装饰、交互像素身份、暂停区及捕获排除列表，只忽略请求revision。像素依Core不可变RasterKey＋bounds约定，不扫描位图；metadata Arc同引用直接相等，不同引用比较完整Object，未复制Session或撤销历史。结构解构保持字段显式，不新增Core公开接口或配置/依赖。

Presenter只记录最后一次成功submit的Arc<AnnotationScene>，重复内容保留原requested/confirmed及waiting起点；不能缓存失败提交或因重复请求延长恢复超时。改变内容仍submit，suspend/close清理snapshot。此前进度测试的空场景实际内容相同，调整fixture使真实版本变化具备新的交互RasterKey，再保持原有迟到确认/覆盖/超时断言，而不是豁免去重。18292 presenter8通过，78707 annotation217通过/5忽略，73906合法场景字段/顺序/参数回归通过；新增提交失败同帧重试、无新请求确认、重复请求不能刷新恢复deadline及Weak证明close释放scene。

审计发现Windows worker已显示静态scene后仅100ms空闲等待，不再检测设备移除；去重不能依赖后续UI请求触发恢复。扩展已有owned-native夹具，在静态画面显示后仅注入一次设备丢失并禁止重建，不submit也不主动wake。38010旧worker回归失败：“idle device loss waited for a new UI scene”。在已有空闲检查点读取device removal状态，将原HRESULT接入现有一次恢复预算及保留旧front buffer协议；有实时帧工作时不新增这次查询。测试注入走相同闲置入口，生产查询使用GetDeviceRemovedReason，不宣称模拟故障等于实际驱动重置。

依据[官方API](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11device-getdeviceremovedreason)，查询返回S_OK或具体移除/重置等HRESULT；核查[微软LowLatencyInput开源样例DeviceResources](https://github.com/microsoft/Windows-universal-samples/blob/main/Samples/LowLatencyInput/cpp/Common/DeviceResources.cpp)，其ValidateDevice检查移除原因并由HandleDeviceLost重建。采用查询与重建机制，未照搬UWP DisplayContentsInvalidated事件、默认adapter变更检查或Present循环；本仓用已有worker闲置等待，不增加线程/高频日志或修改平台通用弹层。

70582原生回归通过，最终97547的owned-native夹具亦通过：没有追加submit的旧stamp进入恢复，以同stamp重建设备后实际像素保持[20,100,180]；devices_created仅增加1，恢复期间只保留原显示，完成后窗口1，clear/join后0。完整夹具同时回归正常预乘RGBA、换会话、一次恢复预算、再次失败软件回退、暂停重建时clear和取消旧frame。100×80实际owned输出截图已查看，只有该测试的纯色显示；不是全App/输入法/双屏实机验证，也未真实注入硬件/驱动移除。

初次workspace通过后Clippy因新增测试中的collapsible_if失败（98423），按门禁停止，未执行Release；已修复并在新增静态健康逻辑后重跑最终门禁。当前early compile仍未跳过，未宣称第四次刷新或端到端显示指标改善。AGENTS.md与边界兼容未改，未暂存/提交/推送；完整App/IME/双屏/资源/物理显示验收继续保留。

最终源码门禁97547 exit0：workspace538通过/59忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（1m10s）和git diff --check通过。原生夹具最终1.64s，截图target/annotation-idle-recovered.bmp已查看，日志target/annotation-static-health-{red,native,native-final}.log、annotation-submit-dedupe-{red,regression,all}.log、annotation-scene-content-regression.log及annotation-submit-dedupe-final-{workspace,clippy,release,diff}.log。


### 菜单清理期间跨窗口输入保护（2026-10-04）

本批继续操作/关闭协议。此前关闭三级菜单只暂停所属二级工具条或水印窗口的原生鼠标接收器；仍存活的其他画布或文字编辑窗口不在保护范围内。新增受控夹具，使移除原生菜单关闭监控时同步回送点击到另一张存活画布。旧实现测试进程异常退出（TLS销毁阶段再次panic，exit 0xc0000409）；日志不包含完整首次panic栈，不把该退出码解释为真实内存越界或实机崩溃证据。代码路径上pointer_canvas在接收回调前通过canvas_current借用Registry，而菜单清理仍持有Registry可变借用，存在重入风险。

现将现有指针暂停机制抽为UI内部PointerGate，窗口与文字编辑实例共用。菜单关闭期间按作用域暂停主工具条、二级面板、水印、全部画布和当前文字编辑的原生鼠标桥接；范围包括样式结算、菜单销毁与关闭后状态更新。暂停只控制同步原生指针交付，不清空队列、不退休存活窗口、不改编辑/焦点/组字内容；作用域退出恢复输入，已退休实例仍由原有生命周期标记拒绝事件。菜单自己的输入仍随销毁退休。不修改平台通用弹层或GPU生产调度，不新增Core接口、配置或依赖。

52846新画布回归通过；40520最终相关标注回归218通过/5忽略，覆盖二级面板与水印菜单关闭、保留队列、关闭后存活画布正常接收，以及编辑暂停后恢复、暂停期间退休后不能重新激活。使用受控Slint组件及同步注入的原生清理回调，不代表实际App/中文输入法/双屏/物理显示验证。日志target/annotation-menu-canvas-{red,green,regression}.log。

性能验收沿用用户修正：60Hz源/144Hz输出优先减少从3到4次刷新的额外等待；约27.783ms P95仍未改善，≤20ms只作参考，另考核超过3次比例、有效FPS及真实操作到物理显示延迟。此次输入保护不宣称改善该GPU尾部。AGENTS.md兼容未改；未暂存、提交或推送，整轮目标仍有实机、资源及性能缺口。

最终源码门禁18015 exit0：workspace539通过/59忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release与git diff --check通过。日志target/annotation-menu-canvas-final-{workspace,clippy,release,diff}.log。


### 相邻显示版本的worker窗口分组（2026-10-04）

本批继续定位额外刷新等待，不改变生产调度。新增仅测试的performance_interval离线模块，消费既有Attempt/Stages、同像素版本CadenceFrame及提交前合成时钟；按正常连续发布、≤3刷新、长间隔且无取整扩大、长间隔正向/负向扩大分组。所有关联在测量结束后执行，未增加逐帧时间查询或实时输出。

以相邻同交换链已显示版本的accepted提交端点限定窗口，裁剪每条既有attempt的elapsed覆盖；重叠记录、缺失接受该提交的唯一attempt或时钟缺失均拒绝归因，保留missing/invalid。未观测空档等于逐样本窗口减已观测attempt覆盖，含线程等待/调度/采集/系统等未知部分，不称为CPU idle、GPU wait或单个API耗时。许可统计采用窗口内开始的attempt；前一提交attempt尾部只计覆盖，不把其此前许可计入本窗口。最终接受attempt用实际包含提交标记的时间区间唯一匹配，不按最近时间猜测。阶段合计为窗口内开始的attempt全部阶段，最终attempt可能在accepted标记后结束，故单独标明不是裁剪时间或该像素版本独占工作；不相加阶段P95。

相关单元回归3通过：覆盖边界裁剪、保留准确许可统计、缺失/重叠/歧义记录拒绝、取整扩大符号、无效周期、具体报告分组、跳号源和不同交换链排除。初次编译因闭包数组类型推导失败，补明确函数指针和参数类型；无依赖更改。51628与63736原生Release测量分别通过，source60Hz/output144Hz，1280×720/强度100模糊/既有完成查询；34822关闭合成时钟探针对照也通过，均35秒左右。第二轮补充窗口内开始attempt阶段合计，只影响离线报告，第一轮没有这些合计字段，不追写旧数据。

| 同口径指标 | 探针第一轮 | 探针第二轮 | 无合成时钟探针对照 |
|---|---:|---:|---:|
| OS有效版本FPS | 57.603 | 58.535 | 58.001 |
| 超过3次刷新比例 | 16.3289% | 15.7265% | 14.6061% |
| 实际刷新计数P95 | 4 | 4 | 4 |
| 同QPC同步间隔P95(ms) | 27.7835 | 27.7835 | 未关联合成时钟 |
| GPU执行P95(ms) | 2.413 | 2.223 | 2.404 |
| 发布调用到OS同步上界P95(ms) | 31.7971 | 31.5277 | 31.4207 |

探针打开的两轮正常连续发布长间隔组共有211/235对：193/221对没有取整额外扩大，18/14对又扩大至少一个取整周期，负向扩大0；全部compose/commit许可阻塞次数0，所有最终接受attempt均是output_retry。此结论只针对这些受控样本，不能证明真实App/其他设备从不阻塞。短组样本1444/1476，未观测空档P95为17.4760/17.3813ms；无取整扩大长组为23.3253/23.2545ms，正向扩大长组为24.6552/21.2352ms。对应已观测elapsed P95分别4.0342/4.1819、4.6301/4.8838、4.4831/5.1863ms。各组分位数不能相加重建某个样本，也不能由空档确定责任模块。

第二轮无取整扩大长组的窗口内开始attempt阶段合计P95：capture3.7060ms、effects0.0538ms、compare0.0269ms；正向扩大长组分别4.1944/0.0407/0.0215ms。都是CPU调用elapsed，effects提交耗时不是GPU滤波执行时间；最终接受attempt的这些阶段均0，因其复用此前已生成的output候选。不支持把第四次刷新简单归为模糊算法昂贵，亦不能据此删除输出一致性确认。

针对未观测空档查阅[PresentMon官方开源项目](https://github.com/GameTechDev/PresentMon/blob/main/README.md)及[Console指标/选项](https://github.com/GameTechDev/PresentMon/blob/main/README-ConsoleApplication.md)：其ETW路径区分CPU/GPU/显示阶段，可按PID与交换链输出时序；下一步需关联worker唤醒、图形就绪与OS显示，保留同像素身份和有/无诊断对照。当前PATH未发现PresentMon/wpr/xperf，未安装或运行额外追踪服务；不据此宣称ETW验证完成。微软[合成统计文档](https://learn.microsoft.com/en-us/windows/win32/api/dcomptypes/ns-dcomptypes-dcomposition_frame_statistics)明确nextEstimatedFrameTime仅为估计，不能当作DWM接收截止线或物理显示。此批不声称生产性能改善，减少第四次刷新等待的目标未达到。

最终门禁26277 exit0：workspace542通过/59忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（测试专用改动后增量0.61s）、git diff --check通过。日志target/annotation-interval-worker-{tests,native1,native2,control}.log与annotation-interval-worker-final-{workspace,clippy,release,diff}.log。未改视觉行为或保存新App截图，完整IME/双屏/资源/物理显示要求继续保留。AGENTS.md兼容未改，未暂存/提交/推送。

### ETW权限核实与内核等待分组（2026-10-04）

上一批同版本分组属于进展。本批核实ETW：当前进程不是管理员，PATH未发现PresentMon/wpr/xperf。GitHub API匿名限流后使用官方release及expanded_assets页面取得v2.6.0便携Console，保存target/annotation-trace-tools/PresentMon-2.6.0-x64.exe。SHA-256与官方资产一致：b2a706bc6ad475749e3b7e3409263aa1e6906d45bdcf993f6dbc0f660188f1af；Authenticode Valid/Intel Corporation。以自身PowerShell PID、独立会话名及1秒时限预检，exit6：failed to start trace session: access denied，要求管理员或Performance Log Users权限。没有取得ETW数据，没有安装MSI/服务、提升权限或修改用户组。来源：[官方发行](https://github.com/GameTechDev/PresentMon/releases/tag/v2.6.0)、[Console选项](https://github.com/GameTechDev/PresentMon/blob/main/README-ConsoleApplication.md)；日志target/annotation-presentmon-{help,preflight}.log。

仅测试新增默认关闭的native_wait_probe：记录WaitForMultipleObjects调用前/返回后Instant、请求时长及实际Signal/Timer返回分支，不含追加记录和CancelWaitableTimer调用。Clock和Mailbox记录容量分别65536，报告丢失；既有attempt记账转移记录，换代清理Clock旧记录，worker退出释放。生产编译无新增记录、时钟查询或日志；HANDLE改为命名字段，创建/等待/取消/关闭语义不变，500us重试策略不变。

离线按同像素相邻accepted提交窗口裁剪真实kernel等待，拒绝与attempt/其他wait重叠。剩余未分类时间为逐样本窗口减已观测work与wait覆盖，各项P95不能相加。Timer超时指标是完整落在窗口内的调用actual-requested非负差的窗口最大值，再取窗口P95；不是每次wait的P95。Signal仅说明共享信号置位，不辨别来源。等待调用包括线程等待和恢复调度，不能直接称GPU活动、捕获通知生成延迟或DWM拾取延迟。

新增回归验证私有kernel event默认不记录、Signal/Timer分支、排空/关闭，以及纯覆盖的边界、Timer超时和重叠拒绝。相关annotation_gpu42通过/30忽略。初次接线误将取Clock记录块放入Port.clear且遗漏Drop取消字段改名，编译失败后移到worker记账并修复。92935首轮通过，74030按等待探针ON→OFF→ON复测通过；1280×720/60Hz DXGI源/144Hz输出/强度100模糊，原完成查询与合成时钟保持一致，每轮约35秒。

| 指标 | ON1 | OFF对照 | ON2 |
|---|---:|---:|---:|
| 等待记录/丢失 | 11120/0 | 0/0 | 11398/0 |
| OS有效版本FPS | 58.503 | 59.937 | 59.032 |
| 超过3次刷新比例 | 15.3934% | 9.1820% | 14.2373% |
| 同QPC同步间隔P95(ms) | 27.7834 | 27.7834 | 27.7836 |
| GPU执行P95(ms) | 2.496 | 3.162 | 2.561 |

ON两组尾部比OFF高，不能声称无扰动或生产收益，探针保持默认关闭。

| 同版本窗口分组 | ON1/ON2样本 | 原未观测空档P95(ms) | kernel等待P95(ms) | 剩余未分类P95(ms) | 窗口最大完整Timer超时P95(ms) |
|---|---:|---:|---:|---:|---:|
| 连续发布≤3刷新 | 1482/1517 | 17.2242/17.1073 | 17.1517/17.0382 | 0.0808/0.0847 | 1.4293/1.5134 |
| 连续发布长间隔、无取整扩大 | 212/205 | 22.4696/22.2719 | 22.3773/22.1894 | 0.1153/0.1309 | 1.5333/1.6076 |
| 连续发布长间隔、正向扩大 | 15/19 | 22.4170/20.2386 | 22.3381/20.1751 | 0.0900/0.1028 | 1.7106/1.3587 |

各组invalid0、负向扩大无样本。短组Signal/Timer返回1768/6483和1806/6868；无取整扩大长组424/1879和412/1829，正向组30/111和38/149。大部分原未分类空档现落在真实kernel等待调用；不能证明等待全都多余或Timer超时一定造成第四次刷新。下一步评估GPU就绪信号减少轮询重试，需单因素对照及候选/像素确认、生命周期回归后决定是否采用。系统ETW、实际App/IME/双屏/资源和物理显示验收仍未完成。

最终99438 exit0：workspace544通过/60忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（1m11s）、git diff --check通过。日志target/annotation-worker-wake-{tests,native1,control,native2}.log及annotation-worker-wake-final-{workspace,clippy,release,diff}.log。没有新App截图，AGENTS.md兼容未改，未暂存/提交/推送。

### 输出比较完成 Fence 唤醒实验：未采纳，已撤回（2026-10-04）

针对500µs轮询可能增加一次刷新等待，做了仅测试启用的输出比较完成通知。Renderer通过ID3D11Device5/ID3D11DeviceContext4创建Fence和自有Event，在输出比较之后、原有Flush之前排入Signal，worker将Event加入等待集合；没有增加Flush或阻塞读回。事件只提示提前重试，原有完成查询、输出比较、冻结候选版本及呈现许可继续决定是否提交；源比较仍沿用原调度。默认关闭，不改变生产行为。

机制参考微软[Signal](https://learn.microsoft.com/en-us/windows/win32/api/d3d11_3/nf-d3d11_3-id3d11devicecontext4-signal)、[SetEventOnCompletion](https://learn.microsoft.com/en-us/windows/win32/api/d3d11_3/nf-d3d11_3-id3d11fence-seteventoncompletion)与[VLC原始Fence实现](https://mailman.videolan.org/pipermail/vlc-commits/2023-April/068216.html)。VLC的无限阻塞等待没有移入本项目；接口可用性和机制参考不等于已验证所有设备的未完成Fence销毁安全性。本实验没有完成全部设备恢复/挂起通知销毁验收，未宣称生命周期已全面证明。

89827初次原生能力、吞吐及像素实验通过；能力用20个递增目标检验事件通知，后续补充的过期事件断言未单独执行，不算已验证。像素夹具覆盖4个源版本、模糊/马赛克各一遍、2个拒绝候选，每帧24个采样点通道最大误差0；不代表整帧或完整App验收。

1234、79557三组baseline→experiment交替原生实测，每轮约35.5秒；1280×720、60Hz DXGI源、144Hz输出、模糊强度100，原比较查询及合成时钟开启，kernel等待探针关闭。下表OS时间关联同一源像素身份，不是物理显示或鼠标输入延迟。

| 指标 | B1 | E1 | B2 | E2 | B3 | E3 |
|---|---:|---:|---:|---:|---:|---:|
| OS有效版本FPS | 58.501 | 58.802 | 58.000 | 58.902 | 59.135 | 58.667 |
| 超过3次刷新比例 | 14.1961% | 13.7266% | 14.5486% | 10.8154% | 12.0135% | 14.7243% |
| 同QPC同步间隔P95(ms) | 27.7834 | 27.7834 | 27.7835 | 27.7833 | 27.7834 | 27.7835 |
| 发布→OS同步上界P95(ms) | 32.6600 | 31.1106 | 32.6810 | 31.3565 | 32.5648 | 31.2002 |
| 观测worker工作(ms/s) | 154.163 | 166.536 | 151.055 | 169.652 | 157.281 | 158.154 |
| GPU执行P95(ms) | 3.140 | 2.526 | 3.284 | 3.384 | 3.352 | 2.527 |

实验Renderer生命周期内收到2566/2601/2570次提示，对照为0，包含预热，不能作为测量窗口内FPS。发布→同步上界三组降低约1.3–1.5ms，但第三组FPS降低、超3次比例增加；工作时间前两组增加约8%/12%，此项不是进程CPU利用率。刷新P95始终约4次，没有稳定减少多出的刷新。按保留条件撤回此次输出通知方案，不能由此推论源比较通知或其他Fence调度都无效。

删除本次新增completion_experiment.rs，移除Mailbox/Renderer、Clock第三等待句柄、测试模式与报告字段，保留此前kernel等待诊断及生产500µs重试。源码扫描无output_fence、completion_experiment、wait_with_event、Wake::Gpu、fence_hints残留。恢复相关测试42通过/30忽略。20743句柄在继续核查时已退休，最终日志确认workspace544通过/60忽略、Clippy和Slint编译通过、Windows Release完成（1m10s）；diff日志仅已有行尾转换提示，继续执行diff check确认退出状态。68777当前恢复源码的Release原生像素回归exit0：8提交/8匹配、4版本×2效果、2拒绝候选、每帧24点、通道误差0。该夹具使用阻塞验证读回，其38–102ms同步区间不能当作worker吞吐或产品输入延迟。

日志target/annotation-output-fence-{baseline1,experiment1,baseline2,experiment2,baseline3,experiment3}.log、annotation-output-fence-restored-{tests,workspace,clippy,release,diff,pixels}.log。下一步分离源比较/取帧等待与输出等待相位；同时继续早期场景编译和交互交接审计。此次无生产性能改善，没有新增App截图；真实操作/IME/双屏/资源及物理显示验收仍未完成，AGENTS.md兼容未改，未暂存/提交/推送。

撤回后补充git diff --check exit0，日志annotation-output-fence-restored-final-diff.log。没有将已退休进程句柄当作运行中等待或重新启动原门禁。

### kernel 等待按前次返回状态分组（2026-10-04）

输出Fence实验不能稳定减少刷新尾部，继续用已有attempt与wait记录做离线归因。performance_interval新增preceding_outcome：以真实wait入口找到最后完成的attempt，将裁剪后的等待时间分到输出重试且Source、源比较且Source、其他Source、其他结果、无唯一前次attempt五类。两个候选同结束时间不猜测；尚未完成的attempt不算前次结果。跨区间边界的等待使用真实入口分类，保留原重叠拒绝规则。

这是“返回状态之后的等待”，不是OS唤醒原因、GPU忙碌时间、具体帧拥有关系或无必要等待。等待期间源到达可改变状态；其他Source包括当前诊断尚未细分的源/输出路径；其他结果包括正常节奏及呈现等，不能一律视为固定节奏。所有分类在测量结束之后进行，未增加实时时间查询或生产调度分支；kernel探针仍默认关闭。新增回归覆盖未完成attempt、结果优先级、剪裁、分组总和等于覆盖时间以及歧义；相关annotation_gpu43通过/30忽略。

65031按ON→OFF→ON三轮原生对照exit0，每轮约35.5–35.9秒，条件与前批一致。OFF的覆盖0仅表示没有记录等待，不代表未发生等待。ON1/ON2记录11429/11471，丢失0；各组invalid0。

| 指标 | ON1 | OFF | ON2 |
|---|---:|---:|---:|
| OS有效版本FPS | 58.136 | 58.370 | 58.301 |
| 超过3次刷新比例 | 11.8187% | 14.0000% | 14.9314% |
| OS QPC间隔P95(ms) | 27.783 | 27.783 | 27.784 |
| GPU执行P95(ms) | 2.709 | 3.074 | 2.558 |

| 同版本窗口分组（ON1/ON2） | 样本 | 全部kernel等待P95(ms) | 输出重试Source之后P95(ms) | 源比较Source之后P95(ms) | 其他Source之后P95(ms) | 其他结果之后P95(ms) |
|---|---:|---:|---:|---:|---:|---:|
| 连续发布≤3刷新 | 1535/1486 | 17.7249/17.3798 | 5.0655/4.6344 | 2.1322/2.1864 | 2.0031/1.9656 | 12.2788/12.7848 |
| 连续发布长间隔、无取整扩大 | 136/198 | 24.1968/22.7948 | 8.7499/8.4746 | 2.8046/3.2461 | 2.3927/2.4199 | 13.5005/14.0096 |
| 连续发布长间隔、正向扩大 | 16/13 | 20.5252/21.4032 | 6.0590/6.1053 | 2.7169/3.3538 | 3.5234/2.4322 | 13.4136/13.0978 |

无唯一前次attempt各组P95为0，负向扩大ON组为空。各列P95不能相加；只有逐窗口的分类总和等于覆盖时间。输出重试之后在长组较高，但其他返回状态也覆盖大量等待；下一步应细分其他状态并关联源到达与常规deadline，不直接将源比较通知或缩短timer作为已证明的解法。ON/OFF刷新尾部有波动，没有无扰动或性能收益声明，第四次刷新目标仍未达到。

最终20343 exit0：workspace545通过/60忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release、git diff --check通过。日志target/annotation-wait-outcomes-{tests,native1,control,native2}.log及annotation-wait-outcomes-final-{workspace,clippy,release,diff}.log。此次保留的源码仅增加测试用离线关联，不新增Core接口、依赖或配置；无App截图，完整实机/IME/双屏/资源与物理显示验收继续保留。AGENTS.md兼容未改，未暂存、提交或推送。

### 细分无等待提示与呈现/暂停/恢复状态（2026-10-04）

前批其他结果之后覆盖约12–14ms等待，尚未辨别是否呈现许可造成。本批保持既有实时诊断和调度，离线将前次attempt的非Source结果拆为None、Presentation、Paused、Recovering，保留全部Source分组和缺失/歧义。匹配穷尽RenderWait变体；扩展原回归验证非Source结果不受残留output_retry/comparison_waiting标志影响，各类裁剪覆盖之和仍等于全部kernel覆盖。相关annotation_gpu43通过/30忽略。

51811 ON→OFF→ON三轮Release原生夹具exit0，每轮35.5–35.9秒；1280×720、60Hz DXGI源、144Hz输出、模糊100及既有完成查询/合成时钟。等待探针ON1/ON2记录11198/11455，丢失0，各组invalid0；OFF未记录等待，0不等于实际没有等待。

| 指标 | ON1 | OFF | ON2 |
|---|---:|---:|---:|
| OS有效版本FPS | 57.367 | 59.233 | 57.902 |
| 超过3次刷新比例 | 15.7558% | 13.7387% | 17.5115% |
| OS QPC间隔P95(ms) | 27.784 | 27.784 | 27.784 |
| GPU执行P95(ms) | 3.054 | 2.557 | 2.971 |

| 同版本窗口分组（ON1/ON2） | 样本 | 无等待提示之后kernel覆盖P95(ms) |
|---|---:|---:|
| 连续发布≤3刷新 | 1445/1429 | 12.4881/12.2519 |
| 连续发布长间隔、无取整扩大 | 176/229 | 15.0144/13.8203 |
| 连续发布长间隔、正向扩大 | 20/14 | 13.8556/11.9585 |

ON2负向扩大仅1个样本（8.5569ms），ON1无此样本，不作为稳健尾部分布。非空组Presentation/Paused/Recovering之后覆盖均0；此前其他结果组在本夹具落到None之后，不支持呈现许可是这段等待主要来源。None只表示最后返回结果没有短重试/暂停提示，不能证明等待全属正常源频率、更不能证明这些等待均可删除。ON尾部相对OFF更高，探针仍默认关闭，不宣称无扰动或生产性能提升。

同时核查到达通知消费与冻结输出交接：run先take_arrival，但render确认旧输出时resolve_output始终传source_waiting=true，确认后仍返回Source；worker执行retry_source，约500µs后再次取源。因此“通知在输出阶段被消费后直接等下一普通tick”的初步假设不符合当前源码。实际运行gpu_single_source_change_survives_unready_surface_and_rejected_commit exit0，包含退休输出须请求prompt latest-source attempt的断言，以及不就绪/拒绝提交不能丢失变化的回归；没有为这个不成立假设修改生产调度。

下一步应将None之后的deadline和源到达相位关联，另建立对象移动请求→匹配OS显示的独立证据。现有gpu_static_source_scene_request_to_attempt只测首次attempt，不是输入到显示；屏幕内容更新结果亦不能替代对象拖动或物理显示。第四次刷新和真实App验收仍未完成。

最终13481 exit0：workspace545通过/60忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release、git diff --check通过。日志target/annotation-wait-hints-{tests,native1,control,native2,source-debt}.log及annotation-wait-hints-final-{workspace,clippy,release,diff}.log。保留变化仅为测试用离线分类，未增加Core接口、依赖、配置或生产探针。无新App截图；IME、双屏/负坐标、资源和物理显示验收仍保留。AGENTS.md兼容未改，未暂存、提交或推送。

### 静止底图上的对象移动与同步隔离审计（2026-10-04）

新增测试专用performance_motion模块，Presentation记录授权场景RenderStamp，按实际交换链/呈现计数关联请求与OS同步，跳过的计数不代替中间版本。移动夹具使用静止底图、1280×720视口、960×480模糊100区域，60Hz提交240次位置变化；保持遮罩纹理尺寸和RasterKey，只平移边界。单独版本色块确保每次请求可见地不同，因此此指标包括额外32×32色块上传，并非无标记纯拖动性能。测量结束后GetPixel检查最终版本色块一个桌面像素，不代表整帧或物理显示验收。请求时间在场景构造/validate之后，故不含UI输入、编译或工具条交接。

初三轮未记录环境的匹配数234/231/223、未观测6/9/17、请求→OS同步上界P95为21.0369/20.3253/28.5041ms，仅作为初步数据。补充环境后发现两轮各2条无效时间快照；原match_commits第一次看到计数便标记已用，即使时间窗口随后不合法也无法恢复。保留失败日志annotation-motion-environment-native1.log及annotation-motion-clock-diagnostic2.log，未放宽时间转换或把无效样本算成有效。

新增match_commits_where复用原唯一计数索引，只有调用方接受的记录才消费计数；旧match_commits保持原行为。移动路径以presented_window合法为前提，拒绝快照计数单列，允许同计数后续有效记录恢复。合法窗口仍必须晚于请求，提交不能早于请求，单Surface限制和重复/歧义拒绝不变。纯回归覆盖跳过计数、不同session、重复提交、无时钟、未来请求和先无效后有效。微软[DXGI_FRAME_STATISTICS](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/ns-dxgi-dxgi_frame_statistics)区分PresentCount/PresentRefreshCount与SyncRefreshCount/SyncQPCTime，不能用读取统计的CPU时刻替代显示时间。初次导入路径及测试对象移动错误已修正；Clippy常量assert失败后改为既有Release检查模式，未绕过告警。

修复后10218三轮原生exit0，实际同一输出报告2560×1440/144Hz，窗口DPI120，场景scale固定1；这是物理尺寸的125%窗口环境，不是100%/125%跨屏场景验证。

| 指标 | 1 | 2 | 3 |
|---|---:|---:|---:|
| 请求版本 | 240 | 240 | 240 |
| 匹配显示版本 | 237 | 229 | 239 |
| 未观测版本 | 3 | 11 | 1 |
| 关联无效/诊断丢失 | 0/0 | 0/0 | 0/0 |
| 无效时钟快照/恢复计数 | 0/0 | 0/0 | 0/0 |
| 请求→OS同步下界P95(ms) | 19.0531 | 22.0727 | 18.5368 |
| 请求→OS同步上界P95(ms) | 19.0532 | 22.0727 | 18.5368 |

三个最终版本均匹配OS计数，桌面色块像素精确一致。此组三轮未自然复现无效后恢复，机制由纯回归验证，不能声称所有失效快照已经现场恢复。未观测版本是没匹配到显示，不猜是丢帧或合并，也不计入延迟P95；不能据此证明全部输入≤50ms或物理显示达标。日志annotation-motion-valid-clock-native{1,2,3}.log；screen源变化27.784ms更新间隔与对象请求延迟是不同指标。

用户提出参照mouse-input-isolation.md隔离无关同步工作。该方向成立为待验证假设，未认定根因：低级钩子曾由UI线程安装，Windows输入必须等安装线程处理消息，是已确认的跨线程阻塞；当前GPU已有独立worker与轻量源通知，Present非阻塞，未发现同一个钩子阻塞模式。仍存在具体耦合：sample_gpu每次枚举所有可见窗口、逐源确认比较后才取得该源新帧；捕获与滤波共用device/context顺序；输出冻结确认期间不采新源；多Surface要求全部许可后提交；commit_current持有mailbox锁执行原生呈现。前两项及锁等待尚需准确测量；多Surface不是当前单屏尾部的解释。已测Commit CPU约0.01ms、GPU约2–3ms，不能将其与刷新量化后的OS区间混称同一个阻塞调用。

下一步以到达→取得最新帧的延迟、源比较完成及枚举成本为切分点，先测再决定是否将最新帧接收与比较/效果消费解耦。不可直接跨线程共享immediate context或删除像素版本确认；跨线程采集早取帧历史实验无稳定收益，须说明新边界区别。目标仍为减少第四次刷新、真实App/IME/DPI/资源及物理显示验收；此批没有生产优化收益声明，未暂存/提交/推送，AGENTS.md兼容未改。

最终1835 exit0：workspace546通过/61忽略、Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和git diff --check通过；相关44通过/31忽略。日志target/annotation-motion-final-{workspace,clippy,release,diff}.log。追加文档后再执行diff check。此前42306因Clippy失败停止，34726为中间源码门禁，均不替代最终源码验证。

## 2026-10-04：窗口枚举同步成本探针

针对 mouse-input-isolation.md 中“隔离无关同步工作”的假设，新增仅测试、默认关闭的 GPU 取帧窗口枚举计时。范围为 windows(request) 的 CPU 侧调用耗时，包含枚举、几何与可见性查询，不包含 WGC 等待、GPU 或 OS 呈现。注入测试帧不沿用上次真实采集计时。生产调度没有改变。

33487 exit0，35秒 owned source 60Hz／1280×720／blur100，枚举探针开→关→开；组合时钟三轮都开，原生等待探针关闭：

| 指标 | 开1 | 关 | 开2 |
|---|---:|---:|---:|
| 枚举样本 | 8690 | 0 | 8743 |
| 枚举单次 P95 ms | 0.7010 | 未测 | 0.6769 |
| 枚举累计调用 ms/秒 | 120.972 | 未测 | 120.701 |
| 可见贡献窗口最大数 | 6 | 未测 | 6 |
| GPU P95 ms | 2.164 | 2.173 | 2.013 |
| 长更新间隔 P95 ms | 27.7838 | 27.7845 | 27.7841 |
| 超过3次刷新比例 | 0.063616 | 0.116499 | 0.079109 |

日志：target/annotation-enumeration-native1.log、annotation-enumeration-control.log、annotation-enumeration-native2.log。关闭的是计时，不是枚举；这些数字不能证明枚举引起或未引起额外刷新。约每秒248次枚举是可疑的重复工作，单次不到1ms仍可能使截止时间跨过一次刷新；需要保持窗口遮挡正确性的独立调度对照才可判断。可见贡献窗口数不是 EnumWindows 总访问数。

参考 [OBS WinRT capture](https://github.com/obsproject/obs-studio/blob/master/libobs-winrt/winrt-capture.cpp)：帧到达回调直接 TryGetNextFrame，再进入图形锁复制纹理。这支持检查取帧时机和锁范围的方向，但不是无需同步的实现，也不能直接替代本项目的多窗口合成与排除自身画布规则。下一步先关联窗口枚举与源到达/错过提交截止时间，再决定隔离窗口目录维护；不把 OS 统计中的间隔统称为一个系统 API 的执行耗时。

本批最终门禁29777 exit0：相关44通过/32忽略；workspace546通过/62忽略；Clippy、Slint build.rs、Windows Release、diff check通过。最终日志为 target/annotation-enumeration-final-{related,workspace,clippy,release,diff}.log。本批只增加测试诊断，没有生产性能收益声明，也未解决额外刷新根因。

## 2026-10-04：冻结窗口目录的单因素对照（已撤回）

上一轮探针发现每秒约121ms窗口查询调用耗时，本轮验证它是否解释多出的一次刷新。仅测试增加冻结目录：第一次GPU请求枚举后复用同一窗口名称/边界/不透明度快照；请求变化立即拒绝；生产构建不含此模式。夹具使用真实 Effects::regions 生成的采集边界，实验组前后分别重新枚举并校验目录签名相同。端点检查不能证明整个期间从未变化，也不能替代动态桌面支持；只是固定 owned source 的成本隔离实验。

48225 exit0，顺序B1→F1→B2→F2，每轮5秒预热+30秒记录、1280×720 blur100、DXGI 60Hz源、OS刷新计数推得约144Hz。GPU/OS时钟和枚举探针均启用，native等待探针关闭。F预热后枚举样本为0，因为目录已经冻结，不表示生产不会枚举。

| 指标 | B1实时查询 | F1冻结 | B2实时查询 | F2冻结 |
|---|---:|---:|---:|---:|
| worker累计调用耗时 ms/秒 | 167.946 | 60.027 | 174.923 | 63.477 |
| 枚举累计调用耗时 ms/秒 | 109.811 | 0 | 114.574 | 0 |
| GPU P95 ms | 2.112 | 2.711 | 2.295 | 2.616 |
| owned源 OS匹配显示FPS | 59.767 | 55.802 | 59.900 | 59.770 |
| 全部有效更新 OS QPC间隔 P95 ms | 27.783 | 27.784 | 27.783 | 27.783 |
| 超过3次刷新比例 | 0.101562 | 0.176330 | 0.105234 | 0.070871 |
| 长间隔子集 P95 ms | 34.7289 | 34.7281 | 27.7845 | 34.7293 |

所有刷新计数非法间隔与源同步延迟非法样本为0，F两轮端点目录校验通过。FPS是匹配相同源像素版本的OS同步，不是物理显示；累计调用耗时不等于进程CPU利用率；长间隔子集P95不能替代全体P95或与其他阶段相加。原始日志 target/annotation-directory-{baseline1,frozen1,baseline2,frozen2}.log；实验源码归档于 target/annotation-directory-experiment-20261004/{capture.rs,annotation_gpu.rs,performance.rs}，用于复查，不在生产代码中保留冻结模式。

结论：省掉重复查询稳定减少约108–111ms/秒的worker调用耗时，但两组刷新尾部方向不一致，整体间隔P95未改善，第一组FPS低于57目标。它没有复现鼠标钩子隔离那样的稳定延迟收益；不能因此断言窗口查询永远无影响，也不能将冻结目录发布为优化。冻结字段、目录缓存、辅助签名函数、枚举模式与ignored测试全部撤回，保留上一轮默认关闭的枚举计时。下一步回到源到达/普通deadline/取帧与提交相位关联，窗口目录隔离不再作为已知延迟修复方案。AGENTS.md无需修改。

撤回后的最终门禁28668 exit0：相关44通过/32忽略；workspace546通过/62忽略；Clippy、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-directory-final-{related,workspace,clippy,release,diff}.log。扫描 crates/lexift-platform/src 未发现冻结实验标识残留；仅保留已存在的源/输出像素版本冻结逻辑。本批产生反证并撤回实验，没有宣称生产性能已改善。额外刷新、真实App/IME/跨屏与物理显示验收仍未完成。

## 2026-10-04：源通知完成后的等待响应

新增仅测试的 performance_wake.rs 离线分析，复用已有源到达事件、实际kernel等待和worker尝试记录；测量循环与生产代码不增加计时或调度。只接受指定owned源、未合并通知、回调进入且通知完成均在同一等待内的唯一事件；排除重叠/逆序等待、非法时钟、多个通知和跨等待返回的通知。报告等待返回以及首个随后尝试，不给通知分配像素版本，也不声称该源一定导致kernel wake。缺失记录与跨边界样本不能解释为响应耗时0。

相关45通过/32忽略；新增纯测试覆盖无记录、重叠等待、其他源、合并通知、跨边界完成、多个候选以及实际再次等待的识别。71886 exit0，两次35秒owned DXGI 60Hz源/144Hz输出/1280×720 blur100，composition与native等待探针均开启（探针并非无扰动基线）：

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 唯一源通知+Signal返回样本 | 1482 | 1598 |
| 通知完成→kernel等待返回 P95 ms | 0.0125 | 0.0128 |
| 通知完成→首个随后尝试 P95 ms | 0.0215 | 0.0211 |
| 首个尝试前再次等待 | 0 | 0 |
| 首个尝试缺失 | 0 | 0 |
| 唯一源通知+Timer返回样本 | 0 | 1 |
| 非法/歧义匹配 | 0/0 | 0/0 |
| 通知完成在等待返回之后（排除） | 1 | 9 |
| 源事件/kernel等待记录丢弃 | 0/0 | 0/0 |
| owned源OS匹配显示FPS | 59.800 | 59.666 |
| 有效更新QPC间隔 P95 ms | 27.783 | 27.783 |
| 超过3次刷新比例 | 0.104294 | 0.084964 |

第2轮唯一Timer样本响应0.0052ms、首个随后尝试0.0088ms；单样本不能推导Timer尾部。日志 target/annotation-wake-response-native{1,2}.log。分析在停止采样后执行，不以最接近的通知冒充同一源帧。

这些匹配样本未显示源通知完成后还被普通deadline挡住，因此下一步转向通知之前的源发布/采集以及取帧后的比较/呈现相位。未覆盖的通知和worker忙碌期间到达仍需分别解释，不能据此宣称整个链路已无等待。另有具体降级路径待处理：Clock不可用时使用Condvar，而源回调只notify、不持有mailbox锁；在检查pending到进入wait之间存在失去条件变量通知的时序窗口。正常Clock使用保留信号状态的内核event，这个降级风险不能解释本次正常路径的27.78ms，也尚未作为修复实现。

最终门禁32900 exit0：workspace547通过/62忽略，Clippy、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-wake-response-{workspace,clippy,release,diff}.log。只新增离线诊断与其回归，不修改生产调度，不宣称性能目标已达成。

## 2026-10-04：定时器不可用时的源通知保留

修复生产 worker 的有限降级等待：Clock 创建/等待失败后，改用原有 auto-reset Signal 的 WaitForSingleObject，而不是 Condvar。旧路径在检查 Notice.pending 与原子释放mailbox进入Condvar等待之间，源回调可以完成notify（回调不持有mailbox锁），通知不会保留。新路径无需让采集回调获取UI/worker锁，预先SetEvent会保留到一次等待消费；多个通知仍合并，pending决定是否提前尝试。[微软事件语义](https://learn.microsoft.com/en-us/windows/win32/sync/event-objects)、[Rust Condvar语义](https://doc.rust-lang.org/std/sync/struct.Condvar.html#method.notify_one)支持这条区别。

scheduler::Signal::wait返回是否由事件唤醒；截止时间向上取整至毫秒，Duration::MAX也限制为INFINITE-1，不意外变成无限等待，500us重试在此降级路径为1ms。worker在等待前释放mailbox锁；event等待失败作为终止错误返回UI，避免错误重试忙循环。正常高精度Clock、60Hz节奏、500us正常重试、无会话/静态场景的既有100ms闲置Condvar检查保持原逻辑；没有全局定时器分辨率设置、新Core接口或依赖。

新增3个非ignored回归：旧Condvar预先通知不保留/新event预先通知保留且只消费一次；零/亚毫秒/大Duration有限超时；禁用Clock并延迟GPU创建的真实worker提交、更新、clear、重新提交与shutdown。最后一项验证调度与生命周期，不冒充GPU或App实测。测试注入字段仅cfg(test)，不进入生产配置。

6932 exit0：新增ignored owned WGC/DXGI原生回归强制整个worker使用event降级，1280×720 blur100、60Hz源、约144Hz输出，5秒预热+30秒采样。event等待调用含预热5038次；owned源OS匹配显示53.636FPS，GPU P95 3.253ms，有效更新QPC间隔P95 34.729ms、超过3次刷新比例0.185945，源同步延迟非法样本0。原始日志 target/annotation-event-fallback-native.log。回归证明降级仍持续处理源像素版本和退出；53.6FPS低于57性能目标，不能把它作为性能达标或正常Clock路径优化。实际屏幕物理显示与完整App/IME/双屏没有在此覆盖。

本修复针对此前已定位的具体降级时序缺陷，不解释正常Clock下约27.78ms的额外刷新；下一步继续查通知前的采集与比较/呈现相位。AGENTS.md 与实现兼容，无需修改。

最终门禁32958 exit0：相关48通过/33忽略；workspace550通过/63忽略；Clippy、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-event-fallback-{final-related,workspace,clippy,release,diff}.log。额外刷新、早期场景编译、完整App/IME/跨屏/资源与物理显示验收仍未完成；不将降级路径修复作为整轮重构完成。

## 2026-10-04：统一原生场景的视口缓存清理职责

本轮先复核WGC时间换算与历史记录。2026-10-03已有独立QPC验证（本文件“WGC时间戳与系统同步关联的时序验证”）：报告时间可以晚于实际取帧，补充capture→dequeue指标因此被拒绝。换算仍在i128域正确处理100ns与frequency，不能把这项不可用指标当作OS执行时间；没有重复该已完成实验。[微软SystemRelativeTime文档](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframe.systemrelativetime)说明它是compositor时间；示例仓库issue127仍无官方结论，不能据其认定普遍WGC缺陷。

转到计划批次3的CPU场景审计，发现render_canvases在compile之前retain_viewports，而compile内部再次对同样的视口构建集合、遍历普通对象/效果/交互/背景/马赛克几何缓存。删除协调器中的前置重复清理；成功原生编译仍由SceneCompiler负责，native_scene为None（后端不可用、窗口token或surface校验失败等）时由软件回退清理。原生提交失败但scene已生成时无需再清理，因为compile已完成此职责。没有新增缓存副本、领域版本、Core接口或依赖，也没有跳过内容校验、像素编译或更改视口集合。

26648 exit0：相关annotation_scene测试12通过/2忽略，含100%/125%像素一致、跨视口旋转文字、去除视口与空视口缓存释放。新增ignored Release CPU对照，固定同一个SceneCompiler、双物理视口、1.0/1.25scale、缓存不变矩形对象，无撤销构建开销；每模式240次，交替顺序。每次same_content校验引用场景的键、层序、bounds、surface与输入策略一致；实际像素由相关场景回归覆盖，不冒充真实App截图。

| 对象数 | 原协调器+编译器两次清理 P95 ms | 编译器一次清理 P95 ms |
|---|---:|---:|
| 1 | 0.001500 | 0.001300 |
| 100 | 0.039800 | 0.038400 |
| 1000 | 0.489500 | 0.438800 |

日志 target/annotation-viewport-cleanup-related.log、annotation-viewport-cleanup-cpu.log。1000对象约减少0.051ms（约10%），仅这一轮稳态CPU夹具；不代表输入到显示、GPU或OS同步改善。它验证了清理职责去重的局部收益，早期编译去重、27.78ms额外刷新与完整App/IME/跨屏/资源/物理显示仍待完成。AGENTS.md无需修改。

最终门禁52302 exit0：workspace550通过/64忽略，Clippy、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-viewport-cleanup-{workspace,clippy,release,diff}.log。未暂存、提交或推送。

## 2026-10-04：呈现授权锁边界的剩余 CPU 时间审计

继续核验 mouse-input-isolation.md 所提示的无关同步耦合。源码中 commit_current 在持有 mailbox guard 时执行 present_surfaces，锁用于防止已被 clear/新场景替换的版本在授权后提交；不能直接移除而留下授权与呈现之间的竞态。Surface 在 GPU worker 创建，同一 worker 每轮 pump_messages 后运行呈现，未发现把该 Surface 的消息循环交给 Slint UI 线程的路径。[微软 Present 文档](https://learn.microsoft.com/en-us/windows/win32/api/dxgi/nf-dxgi-idxgiswapchain-present)确实提醒 Present 可能等待消息线程，因此独立 worker 本身不足以证明调用绝不阻塞，仍需要实测。

新增 cfg(test) performance_commit.rs 的离线计算，不增加测量循环中的时钟、日志、锁或生产路径。每次已接受提交分别以既有 outer commit 时间减去该次所有 Surface 的 Present、DComposition Commit、SetWindowPos 调用时间之和，再计算残差分布；不相减不同指标的 P95。残差包含 mailbox 获取、授权校验、诊断和其他未单独计时工作，只是锁获取时间的保守上界，不是锁持有时间、GPU 或 OS 同步耗时。无已接受调用记录的尝试跳过（其中也包括根本未提交的重试）；溢出或调用和大于外层时间算无效，不钳零。纯测试覆盖配对计算、合法零残差、缺失、逆序、溢出和空集合不可用。

84427、21525 两轮 Release 原生夹具 exit0：各5秒预热+30秒测量，owned DXGI 60Hz源/约144Hz输出/1280×720 blur100；沿用 composition/native等待探针，非无扰动基线。

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 有效已接受提交残差样本 | 2325 | 2357 |
| 无调用记录跳过的尝试 | 10222 | 10317 |
| 残差非法样本 | 0 | 0 |
| 已接受提交外层 CPU P95 ms | 0.297100 | 0.287500 |
| 残差 P95 ms | 0.011600 | 0.011400 |
| 残差最大值 ms | 0.070400 | 0.651400 |
| 残差至少1ms的样本 | 0 | 0 |
| GPU P95 ms | 2.229 | 1.775 |
| owned源 OS匹配显示FPS | 59.834 | 59.469 |
| 有效更新 QPC间隔 P95 ms | 27.783 | 27.783 |
| 超过3次刷新比例 | 0.100892 | 0.125070 |

日志 target/annotation-commit-residual-native{1,2}.log。该夹具没有在提交授权之前发现毫秒级阻塞，暂不以拆 mailbox 锁为性能修复；不排除实际 App 高频 submit/clear 的竞争、未接受尝试或系统异步队列里的其他同步。27.783ms依然是相邻有效显示版本的更新间隔，不是一个 OS API 的调用耗时，也不是当前残差。下一GPU步骤集中于通知前交付以及来源/输出比较顺序，保持同像素版本关联，不继续重复已无稳定收益的窗口目录冻结、空Commit或输出Fence实验。真实App/IME/DPI/物理显示与资源验收仍未完成，整体目标保持进行中。

最终门禁1638 exit0：相关50通过/33忽略；workspace552通过/64忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release和diff check通过。日志 target/annotation-commit-residual-{related,workspace,clippy,release,diff}.log。新增代码仅测试报告和其回归，没有生产性能改善声明；Core接口/依赖/配置及AGENTS.md未改，未暂存、提交或推送。

## 2026-10-04：冻结输出等待期间预取来源的对照（已撤回）

审计发现 source update_texture 的 pending 比较未完成时不取新帧，而 Renderer 在 output Pending 时不调用采集。两者是明确的串行顺序，但不等于已证明造成额外刷新。历史上直接绕过源比较、只保留输出比较的实验已有不稳定结果（见本账本 annotation-latest-source-experiment 的历史记录），本批没有重复它。仅测试新增 prefetch_pending_output：输出比较 Pending 时运行既有 sample_gpu，确认的新区域并入 PendingFrame；不更新效果、不重绘、不覆盖冻结 back buffer。候选 Equal/Changed 后只有 PendingFrame 没有新区域才能清除显示债务，避免将新源误确认成旧输出。没有新增持久帧队列或跨线程GPU操作。

再次核对 [OBS WinRT capture](https://github.com/obsproject/obs-studio/blob/master/libobs-winrt/winrt-capture.cpp)：回调先取帧，随后 obs_enter_graphics 再复制纹理；不能称为无锁路径。[微软 D3D11 多线程说明](https://learn.microsoft.com/en-us/windows/win32/direct3d11/overviews-direct3d-11-render-multi-thread-intro)要求同一 immediate context 串行，并建议 DXGI/Present 在该线程执行，因此本批只试调用顺序，不把同一context并发搬入WGC回调。

49395 exit0，B1→P1→B2→P2，各5秒预热+30秒采样，owned DXGI 60Hz源、约144Hz输出、1280×720 blur100；四轮均开启composition/native等待探针，生产模式默认不变。初期相关50通过/34忽略。

| 指标 | B1原顺序 | P1预取 | B2原顺序 | P2预取 |
|---|---:|---:|---:|---:|
| 输出Pending时额外采集次数 | 0 | 1851 | 0 | 1739 |
| 额外采集中确认变化次数 | 0 | 49 | 0 | 17 |
| worker累计调用耗时 ms/秒 | 173.817 | 188.470 | 178.868 | 207.659 |
| GPU P95 ms | 2.138 | 1.941 | 1.805 | 1.922 |
| owned源 OS匹配显示FPS | 59.737 | 59.669 | 59.837 | 59.765 |
| 有效更新 QPC间隔 P95 ms | 27.783 | 27.783 | 27.783 | 27.783 |
| 超过3次刷新比例 | 0.125628 | 0.128004 | 0.095318 | 0.079799 |
| 同版本取帧→提交 P95 ms | 5.3201 | 7.1397 | 5.3445 | 6.1225 |
| 同版本发布→OS同步上界 P95 ms | 30.8648 | 32.7642 | 30.4110 | 30.8057 |

四轮混合版本提交、未匹配提交与非法QPC更新间隔均0。预取增加CPU调用耗时约8.4%/16.1%，取帧→提交与发布→OS同步上界两组均更大；超过3次比例一组更差一组更好，整体间隔P95不变。未出现稳定整体收益，不能据此保留更复杂的顺序，也不能反推采集/输出串行永远无影响。性能已不满足采用条件，未扩大完整App或像素验收来发布这条路径。

实验字段、配置接线、Pending分支、专用统计、枚举模式与ignored入口全部精确撤回；来源与输出比较仍按原顺序运行，前一批CPU残差诊断保留。日志 target/annotation-pending-capture-{baseline1,prefetch1,baseline2,prefetch2}.log；实验源码留在 target/annotation-pending-capture-experiment-20261004/{annotation_gpu.rs,performance.rs}。扫描 crates/lexift-platform/src 无实验标识残留，不暂存、提交或推送。后续重点是通知前采集交付及共用GPU队列是否存在可测的耦合，不继续重复输出等待预取实验；仍须先证明具体阻塞再决定设备/线程边界，不能将本批间隔称为OS调用耗时或物理显示延迟。

撤回后的最终门禁64833 exit0：相关50通过/33忽略；workspace552通过/64忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（1m09s）和diff check通过，日志 target/annotation-pending-capture-final-{related,workspace,clippy,release,diff}.log。没有永久生产源码变更或性能改善声明；AGENTS.md与计划兼容未改，完整App/IME/跨屏/资源与物理显示验收仍待完成，目标继续进行。

## 2026-10-04：WGC采集帧池单缓冲对照（已撤回）

审计通知前交付与帧占用：生产 CAPTURE_BUFFERS=2，latest_pending 最多消费帧池容量，替换时立即关闭旧lease，update_texture 中在将源复制到自有纹理后归还最终lease；没有将WGC帧保存在Core或跨次渲染队列中。历史三缓冲实验改变的是输出交换链，不是采集帧池。本批仅测试用worker线程局部容量和Mailbox选项在新建pool前选择1或2，WindowCapture记录自身容量供有界取帧和尺寸Recreate使用；滤波、输出交换链与调度未改。实验开关默认关闭，不是新增用户配置。

[微软 CreateFreeThreaded 文档](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.createfreethreaded?view=winrt-26100)说明FrameArrived运行在帧池内部worker，而不依赖DispatcherQueue。[采集指南](https://learn.microsoft.com/en-us/windows/apps/develop/media-authoring-processing/screen-capture)说明帧生命周期归还缓冲，不能在归还后保存底层surface；单缓冲是否降低延迟仍须测量，文档不保证收益。

87092 exit0，双1→单1→双2→单2，各5秒预热+30秒，owned DXGI 60Hz源/约144Hz输出/1280×720 blur100，composition/native等待探针均开启。初期采集相关15通过/6忽略。单缓冲Create、取帧与持续运行在本机API实际成功，不将参数配置报告当作实时池占用测量。

| 指标 | 双1 | 单1 | 双2 | 单2 |
|---|---:|---:|---:|---:|
| GPU P95 ms | 1.936 | 2.262 | 1.658 | 1.973 |
| worker累计调用耗时 ms/秒 | 181.107 | 176.873 | 177.395 | 176.208 |
| owned源 OS匹配显示FPS | 59.836 | 59.902 | 59.870 | 59.666 |
| 有效更新 QPC间隔 P95 ms | 27.783 | 27.784 | 27.783 | 27.783 |
| 超过3次刷新比例 | 0.090301 | 0.136971 | 0.100836 | 0.090553 |
| 同版本取帧→提交 P95 ms | 4.9224 | 5.2794 | 5.3537 | 7.6497 |
| 同版本发布→OS同步上界 P95 ms | 29.4501 | 31.3394 | 30.9526 | 31.5351 |
| 取帧时仍是最新出版内容的比例 | 1.000000 | 1.000000 | 0.999443 | 1.000000 |

各轮owned源均取帧1800次，混合版本提交、未匹配提交及非法QPC更新间隔均0。取帧新鲜度按实际像素ID及完成出版调用关联，不能将它解释为采集真实时刻或物理显示。单缓冲两组GPU、取帧→提交和发布→OS同步上界均更大，超过3次比例方向不一致，整体间隔不变，未满足稳定收益条件，恢复双缓冲。其他桌面来源的更新量有变化（4个采集来源，除owned源之外还有额外到帧），因此不将两组差值一概归因于缓冲数量，也不宣称1或2适用于所有硬件。

线程局部实验函数、Mailbox开关、WindowCapture容量字段、基准模式和ignored入口全部撤回，CAPTURE_BUFFERS及Recreate恢复2；源码扫描无实验标识残留。日志 target/annotation-capture-pool-{two1,one1,two2,one2}.log，实验源码存于 target/annotation-capture-pool-experiment-20261004/{capture.rs,annotation_gpu.rs,performance.rs}。下一步先验证采集来源的贡献范围：windows(request)按边界相交收集窗口，没有按上层覆盖裁剪；模糊采样边缘又可能真实需要底层内容。不能简单排除所有底层窗口，应先让自有源完整覆盖滤波边缘并验证像素，再做单源与多源交付隔离对照，判断额外来源是否产生不必要的同步/唤醒。系统合成内部原因和真实App/资源/IME/DPI/物理显示仍未解决。

撤回后的最终门禁57533 exit0：采集相关15通过/6忽略；workspace552通过/64忽略，Clippy --workspace --all-targets -- -D warnings、Slint build.rs、Windows Release（1m08s）和diff check通过，日志 target/annotation-capture-pool-final-{related,workspace,clippy,release,diff}.log。没有永久生产代码变更或性能改善声明，Core接口/依赖/配置及AGENTS.md未改，不暂存、提交或推送；整体目标保持进行中。

## 2026-10-04：完整采样覆盖下的来源隔离与 WinRT 工厂退休修复

沿 mouse-input-isolation.md 的无关同步依赖思路，先构造可证明像素贡献的夹具：1280×720 自有 DXGI 源、896×336 内缩效果遮罩，实际滤波计划及网格对齐后的采样矩形全部包含于源内。仅测试排除快照中的其他窗口，不改变生产窗口选择规则。负坐标纯测试验证完整采样范围；冻结输出读回对照逐字节比较两种来源策略，Blur/Pixelate 各3,686,400字节最大差0，并校验六处已知源颜色，避免透明输出也通过。该读回不是实际App截图或物理显示证据。

首次原生像素回归在 Blur 对照后以0xc0000005崩溃（target/annotation-source-scope-pixels.log）。本机dump异常为读取已卸载 GraphicsCapture.dll 中的工厂vtable，反汇编call [rax+30h]与CreateFreeThreaded ABI及格式/缓冲参数一致。windows 0.62.2生成的IsSupported/CreateFreeThreaded使用进程静态factory cache，工厂缓存生命周期超过显式RoUninitialize。新screen_capture::activation用局部RoGetActivationFactory及公开ABI调用，工厂在所属apartment内释放；支持查询可借用调用线程现有STA且不改变其初始化计数，Backend仍严格拥有MTA。不长期固定DLL、保留隐藏窗口或增加依赖。此为生命周期修复，不能当作额外刷新根因。

94637原生回归通过：完整apartment退休并重新获取支持与pool八轮（1.22s），Blur/Pixelate全输出像素对照（1.11s）。日志 target/annotation-source-activation-native.log、annotation-source-scope-pixels-fixed.log；新STA普通回归通过。首次崩溃保留为失败，不重写成通过。

95943 exit0，全部相交来源→仅自有源→全部→仅自有源，各5秒预热+30秒采样，60Hz源/约144Hz输出，composition/native等待探针均开启。同一新工厂实现、1280×720视口及896×336遮罩，不与历史满屏blur100夹具直接比较。仅自有源模式断言采集数量1且回调来源身份正确。

| 指标 | 全部1 | 自有1 | 全部2 | 自有2 |
|---|---:|---:|---:|---:|
| 采集窗口数 | 4 | 1 | 4 | 1 |
| GPU P95 ms | 1.205 | 2.135 | 1.797 | 1.370 |
| worker累计调用耗时 ms/秒 | 163.505 | 73.950 | 153.078 | 73.491 |
| owned源OS匹配FPS | 57.899 | 59.836 | 55.268 | 59.933 |
| 有效更新QPC间隔P95 ms | 27.783 | 27.783 | 27.784 | 27.783 |
| 超过3次刷新比例 | 0.089862 | 0.078595 | 0.136391 | 0.073456 |
| 同版本取帧→提交P95 ms | 5.0753 | 5.2782 | 5.0249 | 5.0988 |
| 发布→OS同步上界P95 ms | 28.1225 | 28.0196 | 27.9999 | 28.1586 |

日志 target/annotation-source-scope-{all1,owned1,all2,owned2}.log。隔离减少worker调用成本约52–55%，有效吞吐两轮均提高，超过3次比例均下降，但间隔P95不变，端到系统同步无稳定下降；全部2低于57FPS目标。GPU span含feed gaps、worker耗时非进程CPU占用、各阶段P95不能相加。SystemRelativeTime未来时间问题仍使capture_clock指标无效，不用该指标论证本批收益；同像素提交混合/未匹配、QPC间隔非法及诊断丢弃均0。

额外来源有成本已得到支持，生产策略仍需覆盖透明窗口、圆角、滤波边缘及遮挡变化，不能只按窗口矩形覆盖就排除底层。下一GPU步骤先建立安全贡献判断与同GPU队列时序证据；第四次刷新原因、早期CPU场景编译以及完整App/IME/跨屏/资源/物理显示仍未完成。AGENTS.md与DAG兼容无需修正，未暂存、提交或推送。

最终门禁82009 exit0：采集相关16通过/7忽略；workspace554通过/68忽略；Clippy --workspace --all-targets -- -D warnings、Windows workspace Release（1m09s）及diff check通过。UI编译由既有build.rs集成，本批未修改Slint。日志 target/annotation-source-final-{related,workspace,clippy,release,diff}.log。测试报告现仅在完整覆盖夹具中输出896×336采样口径，避免给历史满屏夹具错误标注。本批生命周期修复及诊断保留，整体重构目标继续进行。

## 2026-10-04：未变化场景在组装前精确复用

继续计划批次3：此前Presenter在SceneCompiler完成后才判断same_content，重复请求仍组装完整层数组、遍历视口缓存并校验交互资产。SceneCompiler现以最后一次成功编译的完整输入先做精确比较，命中后共享既有AnnotationScene资产，只更新请求stamp。比较对象实际值与顺序，不用指纹碰撞作为正确性条件，不复制Session、撤销历史或长轨迹。

新增UI内部annotation_scene_reuse：对象快照借用已有栅格/效果缓存中的Arc；普通ObjectRaster改为Arc，使编译输入与资产缓存共享快照。未缓存的屏幕外对象令整段复用保守失效，不新增完整文档副本。比较涵盖会话、窗口token/排列/物理视口/DPI、编辑投影后的对象及选中状态、草稿、悬停、参考文本、光标/选区/组字反馈、水印/显示布局/聚光不透明度、暂停区域与采集排除项。输入变化、失败、软件回退视口清理及clear均清除旧组装缓存；生产仍由原编译器生成新内容，Presenter保留提交去重及旧stamp确认规则。

21106相关回归15通过/3忽略：28类输入变化逐项对照全新编译的正文及交互像素，覆盖负坐标、1.0/1.25 DPI；效果强度/类型变化回写实际metadata而不误复用规范化白色遮罩，未变mask保留；两千点轨迹仍只保留一个栅格快照缓冲，组装状态增加Arc引用而不复制点；无效输入、视口退休、clear释放快照与像素。首次新增测试编译失败属于夹具Session无Clone及闭包借用生命周期，已修正，失败日志不作为通过证据。

原viewport cleanup基准改为直接调用assemble，避免新增整场景命中令旧清理对照混入另一个优化。新增CPU夹具分别对比既有组装体与精确复用，固定双视口及缓存资产、交替执行，像素正确性由上述回归覆盖；不宣称GPU、真实App输入或物理显示收益。Core接口、配置、依赖及AGENTS.md不变，未暂存、提交或推送。

最终源码CPU对照62106中的ignored Release基准通过（编译2m38s、执行0.52s）：双视口1.0/1.25、固定不变矩形资产、每模式240次交替，夹具直接构建对象数组而不构建撤销历史；各次同内容/完整metadata一致且复用层数组Arc身份不变。

| 对象数 | 原组装体P95 ms | 完整输入精确复用P95 ms |
|---|---:|---:|
| 1 | 0.001800 | 0.000100 |
| 100 | 0.035400 | 0.000700 |
| 1000 | 0.496000 | 0.012800 |

日志 target/annotation-scene-reuse-cpu-final.log；首次31746基准仍保留于annotation-scene-reuse-cpu.log，但其夹具通过Session.add建立撤销/身份，与最终直接数组夹具不同，不把两组当同条件重复样本。命中仍需O(对象字段/点数)精确比较，并非所有画布请求零成本；变化场景组装/首次栅格化和GPU采集线程未在此计量。缺少屏幕外对象缓存时不命中是明确剩余优化范围。

62106后续workspace557通过/69忽略；Clippy拒绝测试夹具对cfg!(debug_assertions)的常量断言，后续Release门禁未执行。已改为与既有基准相同的if+panic Release运行保护，不改变CPU测量路径；修复后重新运行相关→workspace→Clippy→Windows Release→diff门禁，不把62106整条门禁记为通过。

最终门禁32729 exit0：annotation_scene相关15通过/3忽略；workspace557通过/69忽略；Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成编译、Windows workspace Release（2m16s）及diff check通过。日志 target/annotation-scene-reuse-final-{related,workspace,clippy,release,diff}.log。CPU对照发生于相同生产路径，随后仅改测试Release保护写法；没有为该保护重复昂贵原生/性能测试。新复用模块与测试已按领域分离，不修改UI样式。整体仍需变化场景跨视口准备、第四次刷新归因、安全来源贡献裁剪及真实App/IME/DPI/资源/物理显示验收，不宣称整轮重构完成。
## 2026-10-04：像素证明来源裁剪实验（性能未通过，已撤回）

实验由GPU比较采样ROI的alpha，只允许已完成、已出版且全不透明的纹理排除下层贡献；全局透明度须255，证明限制在ContentSize及出版边界。位置、ROI或纹理变化使证明失效；静止源使用已拥有的完整纹理重新证明。没有CPU位图扫描或以窗口矩形代替不透明证明。最终生产不保留此路径。

82221原生alpha测试通过，覆盖半透明、末像素孔洞、ROI外孔洞、分离ROI、空/越界范围及未完成证明。Blur/Pixelate各3,686,400字节最大误差0。初始55071生命周期测试在移回静止窗口时失败：证明撤销后没有新帧，采集数停留2；修正静止重新证明后71145通过，透明、恢复不透明、移开、移回四阶段采集数2/1/2/1，各ROI1,204,224字节最大误差0，停止后采集数0。既有GPU比较与采集回归也通过。日志 target/annotation-occlusion-{opacity-native,pixels-native,lifecycle-native,lifecycle-native-fixed,compare-native,capture-native}.log。自有DXGI窗口与纹理读回不替代完整App/IME/跨DPI/物理显示，真实逐像素分层和圆角窗口未实机覆盖。

66986 exit0，未裁剪→裁剪→未裁剪→裁剪，每轮5秒预热+30秒测量，60Hz源/约144Hz输出，1280×720源及896×336完整采样覆盖遮罩。两侧共用实验alpha kernel和从上到下采集顺序，仅裁剪开关不同；未裁剪不是旧生产代码的无扰动基线。

| 指标 | 未裁剪1 | 裁剪1 | 未裁剪2 | 裁剪2 |
|---|---:|---:|---:|---:|
| 采集窗口数 | 4 | 1 | 4 | 1 |
| GPU P95 ms | 1.727 | 2.067 | 1.376 | 2.068 |
| worker调用耗时 ms/秒 | 171.334 | 132.123 | 158.192 | 131.672 |
| owned源OS匹配FPS | 59.067 | 56.602 | 57.335 | 55.634 |
| 有效更新QPC间隔P95 ms | 20.838 | 27.783 | 27.783 | 27.784 |
| 超过3次刷新比例 | 0.044608 | 0.120212 | 0.082024 | 0.151679 |
| 同版本取帧→提交P95 ms | 4.852 | 5.425 | 4.690 | 5.487 |
| 发布→OS同步上界P95 ms | 28.2657 | 28.6896 | 28.2672 | 28.7244 |

worker成本下降约22.9%/16.8%，但两组FPS下降且裁剪低于57，GPU和两种时延指标更大，超过3次比例升高，未满足采用条件。四轮混合/未匹配提交及出版/呈现记录丢弃均0；WGC未来时间仍令capture_clock不可用。提交约60FPS不能替代显示FPS，阶段P95不相加，单轮20.838ms不能证明额外刷新已解决。

日志 target/annotation-occlusion-{baseline1,pruned1,baseline2,pruned2}.log，实验源码留 target/annotation-occlusion-experiment-20261004。恢复原kernel、16字节元数据、从下到上采集及来源独立确认，移除实验不透明状态、模块、开关与专用回归；保留WinRT工厂退休修复、来源贡献夹具和场景精确复用。无Core接口、配置或依赖变化，无暂存、提交或推送。

下一步分别隔离额外来源通知与实际GPU工作/完成等待：退休来源同时减少会话、通知和GPU任务，本批尚不能判断哪项导致差异。降低CPU成本不保证及时呈现。系统合成根因及完整App/资源/IME/DPI/物理显示仍未完成。


Rollback gate 83881 exit0: screen_capture 16 passed/7 ignored; workspace 557 passed/69 ignored; Clippy --workspace --all-targets -- -D warnings, UI build.rs/Slint, Windows workspace Release (1m10s), and git diff --check passed. Logs: target/annotation-occlusion-rollback-{related,workspace,clippy,release,diff}.log. Source scan found no CAPTURE_OPACITY, disable_opaque_pruning, CoveredUnprunedSources, comparison_for or occlusion module references.

通知实验最终门禁56922 exit0：相关16通过/7忽略，workspace557通过/69忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release（1m09s）及diff check通过。日志 target/annotation-wake-isolation-final-{related,workspace,clippy,release,diff}.log。最终源码无通知筛选标识，只有离线报告计数；随后新增观测覆盖诊断属于下一批，不以本门禁代替。

## 2026-10-04：保留全部来源，仅隔离额外唤醒（无收益，已撤回）

为分开上一批同时改变的来源退休和通知，本批只增加cfg(test)主来源通知筛选。保留全部捕获会话、采集顺序、双帧池、纹理复制和比较/合成策略；额外FrameArrived仍记录，但不置pending、不发Signal/Condvar，不抢占主来源通知。生产回调没有新增筛选、计时或日志。纯测试覆盖额外通知前后均不消耗主来源事件、主来源重复通知合并及诊断完整；初始相关17通过/7忽略。

99723 exit0：全部唤醒→仅主来源唤醒→全部→仅主来源，各5秒预热+30秒测量，沿用完整覆盖ROI夹具、60Hz源/约144Hz输出、原生等待与composition探针。四轮都保留4个采集来源；筛选两轮断言额外到帧存在且额外唤醒为0。实际尝试和GPU执行量可随唤醒节奏变化，保留的是相同处理策略而非声称执行计数相同。

| 指标 | 全部1 | 主来源1 | 全部2 | 主来源2 |
|---|---:|---:|---:|---:|
| 额外来源到帧通知 | 960 | 985 | 932 | 1147 |
| 额外来源实际唤醒 | 954 | 0 | 925 | 0 |
| worker调用耗时 ms/秒 | 160.958 | 142.526 | 158.448 | 137.764 |
| GPU P95 ms | 1.361 | 1.977 | 1.213 | 1.822 |
| owned源OS匹配FPS | 56.435 | 55.702 | 56.301 | 54.968 |
| 有效更新QPC间隔P95 ms | 27.783 | 27.783 | 27.783 | 27.784 |
| 超过3次刷新比例 | 0.115248 | 0.127545 | 0.108412 | 0.140777 |
| 同版本取帧→提交P95 ms | 5.066 | 5.399 | 4.849 | 5.212 |
| 发布→OS同步上界P95 ms | 28.7036 | 28.7782 | 28.2940 | 28.7127 |

两组worker成本下降约11.5%/13.1%，FPS及尾部均恶化，未支持以屏蔽额外唤醒修复第四次刷新。四轮均低于57FPS目标，P95仍4次刷新；不能将通知增加称为普遍性能优化。其他来源到帧数量并不完全相同，数据来自同机交替夹具而非系统内部因果跟踪。四轮混合/未匹配提交、capture/呈现记录及native等待记录丢弃均0；WGC未来时间指标仍不可用。阶段P95不相加，OS同步不等于物理显示，也不替代真实App。

日志 target/annotation-wake-isolation-{baseline1,owned1,baseline2,owned2}.log，源码归档 target/annotation-wake-isolation-experiment-20261004。筛选字段、方法、worker接线、枚举模式、专用入口与纯回归全部撤回；仅保留基准报告离线统计额外到帧/实际唤醒次数，便于后续明确环境差异，不改变测量循环或生产行为。

再次核对[OBS回调](https://github.com/obsproject/obs-studio/blob/master/libobs-winrt/winrt-capture.cpp)与[Win32CaptureSample](https://github.com/robmikh/Win32CaptureSample/blob/main/Win32CaptureSample/SimpleCapture.cpp)：OBS取帧后进入graphics再复制，样例使用free-threaded pool且在回调复制纹理。它们不证明共享GPU工作能无锁并发，也不提供本项目多来源确认、像素合成和退休授权的直接替代。此前回调提前取帧实验已无稳定收益，不重复。下一步结合已有同版本记录检查比较确认与系统合成相位的耦合；需要系统队列证据时明确ETW权限缺口，不将未定位原因归为系统极限。AGENTS.md与本批没有冲突，无暂存、提交或推送。

## 2026-10-04：区分连续呈现计数与观测空洞

GetFrameStatistics只报告某个呈现计数；match_commits不会把跳跃计数中间的提交猜测为已显示。原报告对已观测独立来源版本统计FPS与相邻QPC间隔，但未区分原生计数连续性。新增仅测试的performance_coverage离线分类，连续同链PresentCount差1与计数跳跃分开统计，报告未知中间提交数量。链替换、逆向/半范围歧义、无效QPC单独拒绝；u32回绕保持相邻计数语义。纯测试通过。第一次接线引用不存在的optional_p95导致编译失败，补齐空集合返回unavailable后测试及原生测量通过。

47974 exit0，两个全部来源夹具各5秒预热+30秒，原采集/呈现逻辑未改变，分析仅在测量结束后执行。不把不同轮次表现视为生产性能优化。

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 已观测来源版本FPS | 59.835 | 56.834 |
| 全体已观测相邻QPC间隔P95 ms | 20.838 | 27.783 |
| 原生计数连续间隔 | 1791 | 1611 |
| 连续计数组QPC间隔P95 ms | 20.838000 | 27.782600 |
| 连续计数超过3次刷新 | 18 | 88 |
| 计数跳跃间隔 | 3 | 93 |
| 跳跃计数组QPC间隔P95 ms | 34.728000 | 34.729300 |
| 未观测的中间呈现计数 | 3 | 93 |
| 无效分类 | 0 | 0 |

日志 target/annotation-observation-coverage-native{1,2}.log。计数跳跃的中间提交可能是重复来源、未被轮询观测到的显示或被丢弃提交，不能直接称丢帧；报告FPS是已观测匹配版本的下界，不是全部真实显示帧率。连续计数间隔也出现第四次刷新，第二轮连续组P95仍27.783ms，说明观测空洞不能完全解释原指标；也不能单凭这些间隔把额外等待定位于某个OS调用。两轮差异提示环境/相位波动，局部CPU改善或单轮20.838ms都不构成总体达标。

新模块只读取已有Observation数组；没有新增热路径时钟、线程、锁、GPU查询或生产逻辑。后续分组可以限制到连续原生计数，继续追踪同版本采集确认/提交/合成关系；仍保留完整App、IME、跨DPI、资源与物理显示验收缺口。

最终门禁46201 exit0：coverage纯测试1通过；workspace558通过/69忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release及diff check通过。日志 target/annotation-observation-coverage-{related,final-workspace,final-clippy,final-release,final-diff}.log。未暂存、提交或推送，目标仍进行中。

## 2026-10-04：连续呈现计数内的阶段扩大分类

在既有performance_coverage中增加同一像素版本对的阶段分类，仅接受同一swapchain相邻PresentCount差1，保留原数组邻接而不先删掉跳跃帧再拼接。源编号连续且发布间隔不超过三次刷新预算时，分别统计短/长间隔的四阶段间隔与三段有符号增长；增长是后一阶段相邻版本间隔减前一阶段同一版本对间隔，不是单帧耗时，也不相减不同P95。纯回归覆盖计数空洞不得桥接、源编号跳跃排除及增长定位；相关2通过。

44766 exit0，两轮全部来源原生夹具各5秒预热+30秒，生产采集/呈现及测量循环不变，分析在采样后执行。

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 已观测来源版本FPS | 58.066 | 52.333 |
| 全体相邻QPC间隔P95 ms | 20.839 | 34.729 |
| 连续原生计数且四端点合法的版本对 | 1694 | 1340 |
| 源连续且发布在预算内的长版本对 | 33 | 20 |
| 最大正增长在发布→取帧 | 10 | 5 |
| 最大正增长在取帧→提交 | 7 | 2 |
| 最大正增长在提交→系统同步 | 16 | 13 |
| 长组发布间隔P95 ms | 17.9451 | 18.1007 |
| 长组取帧间隔P95 ms | 32.0998 | 23.3479 |
| 长组提交间隔P95 ms | 49.5622 | 27.9788 |
| 长组系统同步间隔P95 ms | 41.6749 | 34.7284 |
| 发布→取帧增长P95 ms | 16.1992 | 7.2964 |
| 取帧→提交增长P95 ms | 27.8853 | 6.3665 |
| 提交→同步增长P95 ms | 11.8236 | 10.3290 |

日志 target/annotation-consecutive-cadence-native{1,2}.log。上述各行P95不相加；最大增长是同一版本对的分类，不表示该段是唯一原因或OS/GPU活跃耗时。两轮分布均不是单一阶段，提交→同步最多，但发布→取帧和取帧→提交也有贡献。第二轮52.3FPS低于目标且整体相位/环境波动明显，不能以本批宣称达标或生产优化。已观测FPS是匹配观测下界，OS同步不是物理显示。

后续不要再用全体27.783ms统一归因为某个OS函数；针对连续计数的提交→同步长组查同设备队列和合成取批边界，同时保留上游扩大分类。现有查询不能直接看到DWM取批截止时刻，ETW权限缺口依然存在；真实App交互、资源、IME/DPI及物理显示要求仍保留。无新增热路径时钟/查询/日志、Core接口、依赖或配置，AGENTS.md无需修改。

最终门禁96157 exit0：相关coverage2通过；workspace559通过/69忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release及diff check通过。日志 target/annotation-consecutive-cadence-{related,final-workspace,final-clippy,final-release,final-diff}.log。仅保留离线诊断，未暂存、提交或推送；整体目标未完成。

## 2026-10-04：实际设备与交换链排队配置核验

审计Renderer::new：Graphics使用Backend的设备/context clone，WGC纹理、效果与交换链没有新增Lexift设备间共享/复制边界。Surface从同一device创建composition交换链及DComposition visual，最大帧延迟已设置1，双缓冲FLIP_SEQUENTIAL、预乘alpha和FRAME_LATENCY_WAITABLE_OBJECT；present使用DO_NOT_WAIT，原生窗口由同一GPU worker创建与pump_messages处理。输出比较完成后resolve_output直接授权present，没有新增回读或纹理复制。本结论是项目源码中的路径，不排除WGC、驱动或DWM内部设备/队列依赖。

现有owned桌面像素夹具增加仅测试的verify_device_and_queue：以IUnknown身份核对交换链、back buffer和immediate context实际所属device，读回GetMaximumFrameLatency、GetDesc1并核对buffer/flip/alpha/waitable标志，查询Surface HWND所属线程与当前线程一致；同时确认capture与Graphics的device/context实例相同。查询位于4版本像素夹具开始前，不进入吞吐测试或生产路径。

26638 exit0：相关annotation_gpu53通过/36忽略；Release原生夹具0.27秒通过，native最大帧延迟1、双缓冲2、设备身份及线程身份匹配，四次连续版本[7,63,127,211]的owned桌面采样色均精确匹配。日志 target/annotation-presentation-topology-{related,native}.log。只验证当前机器自有窗口，不能替代实际App、双屏或物理显示，也不能把SetMaximumFrameLatency(1)解释为DWM/整个GPU队列深度1。

核对[WaitForCommitCompletion官方语义](https://learn.microsoft.com/en-us/windows/win32/api/dcomp/nf-dcomp-idcompositiondevice-waitforcommitcompletion)：它等待合成引擎处理前一个Commit，不等于物理显示。没有证据支持为了降低延迟向生产加入这项同步等待。当前证据排除夹具中的额外Lexift设备、错误默认排队配置和外部消息线程依赖；下一步仍需定位提交之后的具体取批/队列等待，不能用另起设备或盲目提高线程优先级代替归因。AGENTS.md依赖DAG与当前架构兼容，无需修改。

最终门禁50539 exit0：workspace559通过/69忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release（1m07s）及diff check通过。日志 target/annotation-presentation-topology-final-{workspace,clippy,release,diff}.log。新增查询只在原生验证中调用，没有生产性能收益声明；未暂存、提交或推送，完整目标仍未完成。

## 2026-10-04：变化普通对象跨视口共享边界准备与快照

继续批次3。ordinary_document在每个视口缓存未命中时原先各自执行stroke_points扫描/插值、说明及链接箭头边界计算，并clone对象快照。长笔迹修改导致两个视口分别准备相同几何和轨迹副本。将原content_bounds拆为视口无关geometric_bounds和原公式的DPI留白/视口裁剪；layers的一次组装使用私有PreparedObject map按原ObjectId/draft索引身份共享未裁剪边界及Arc快照。所有缓存命中仍在准备前返回；不同视口仍独立裁剪、DPI绘制及保存栅格，绘制坐标和层序不变。

PreparedObject只属于同步组装栈，不新增长驻文档缓存或Core接口。快照延迟到首次可见栅格成功分配后创建，屏幕外对象仅共享边界、不复制轨迹；组装完成/失败后释放临时map，可见视口缓存共享快照。clear/视口退休继续释放既有缓存。仅共享普通对象准备，不修改马赛克、放大镜、背景、水印或交互排版规则。

90982 exit0：相关annotation_scene17通过/4忽略。新独立回归覆盖Pencil/Polyline/Arrow连续四次修改、两个负坐标接缝视口100%/125%、视口顺序反转；合成像素与独立render_editing各方向误差≤1通道值，两个缓存的对象Arc身份相同，保留轨迹缓冲1份，clear后Weak失效。屏幕外准备无对象快照。既有普通/大字号旋转文字、马赛克、说明、撤销/编辑投影与全输入失效回归仍通过。

Release CPU配对对照：10万点、两个视口，80轮交替先后顺序，前16轮预热、64轮各路径样本。参考路径每视口计算边界与clone快照，共享路径准备一次、按各DPI裁剪并共享快照；两视口边界相同。准备P95为0.605400ms→0.314000ms，轨迹快照2份→1份。日志 target/annotation-scene-preparation-cpu.log。测量是边界和快照准备，不含HashMap查找、栅格绘制、完整SceneCompiler、GPU、实际App或物理显示，不能将约48%局部下降换算成跟随延迟改善。窗口外准备和单视口没有相同双视口收益保证。未暂存、提交或推送，AGENTS.md/DAG无冲突。

最终门禁41256 exit0：相关scene17通过/4忽略；workspace561通过/70忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release（2m12s）及diff check通过。日志 target/annotation-scene-preparation-final-{related,workspace,clippy,release,diff}.log。90982 Release CPU对照通过；本批生产共享准备已保留，但不证明GPU/完整App/物理显示达标。整体目标继续进行。

## 2026-10-04：箭头与折线跨视口共享插值中心线

继续批次3。上批共享边界和快照后，箭头/折线绘制仍在每个视口独立调用stroke_points进行曲线插值。本批在PreparedObject中延迟保留中心线，仅在首次可见栅格分配成功后创建，同次layers组装中其他视口复用；屏幕外准备不保留中心线，组装结束释放临时向量。Pencil/HighlightLine原绘制已借用点集，未改动；文字/序号链接箭头继续走原包装入口。路径仍使用各视口局部坐标创建，描边、填充、虚线及端点构造算法不变，避免共享变换路径改变抗锯齿。

76235 exit0：annotation_scene相关18通过/5忽略。新增回归覆盖曲线Arrow/Polyline、五种虚线、填充及不同起止端点、点集连续修改、负坐标接缝100%/125%两个视口；合成像素与独立render_editing最大通道差≤1。屏幕外准备同时验证快照和中心线为空；Release夹具验证共享内容与Core结果相等、重复读取地址一致。

仅CPU的1万控制点双视口夹具，80轮交替顺序、前16轮预热，插值约119989点；边界/快照/插值准备P95原路径0.668300ms、共享0.522800ms。边界自身仍插值一次，绘制中心线两次改为一次，因此总插值3→2，未声称3→1。日志 target/annotation-line-preparation-{related,cpu}.log。夹具不包含map查找、路径构建、栅格、完整App、GPU或物理显示，不能据此推断额外刷新已减少。无Core接口、配置或依赖变化，AGENTS.md/DAG无冲突；未暂存、提交或推送。

最终门禁29573 exit0：workspace562通过/71忽略；Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成编译、Windows workspace Release（2m09s）及diff check通过。日志 target/annotation-line-preparation-final-{workspace,clippy,release,diff}.log。曲线准备优化已保留，GPU额外刷新归因、完整App/IME/DPI/资源/物理显示验收仍未完成，整体目标继续进行。

## 2026-10-04：额外来源比较未确认期间的独立提交证据

沿鼠标隔离的依赖审计思路检查sample_gpu：已有纹理时，单来源Outcome::Pending仅保留该来源基准和comparison_waiting，不使complete=false；主来源已确认变化可进入效果/输出。只有来源没有首帧纹理时才等待完整采样，不能省略它而破坏透明和边缘合成。本批不改变生产行为，新增仅测试performance_source_wait，采样结束后读取已有capture_events与source_presentations；为其他来源构造同token/sequence、同Confirm.started生命周期中的Pending→Confirm观测区间，统计区间内接受的独立主来源版本。缺少确认、旧生命周期、版本不匹配、混合主版本及重复提交不计，暖机与未完成尾部保守排除；统计不新增热路径计时、GPU查询或日志。

初次纯测试使用RenderStamp::default导致编译失败，改为明确session/revision后通过；同时按token/sequence索引Pending，避免对每次确认全量扫描事件。纯回归验证区间边界、候选身份、生命周期、未确认排除、主来源排除及去重。

59775 exit0，两轮全部来源完整覆盖夹具，各5秒预热+30秒，60Hz源/约144Hz输出，生产采集/比较/提交与已有探针保持原样。

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 捕获来源数量 | 4 | 4 |
| 其他来源已确认的Pending区间 | 41 | 70 |
| 主来源独立接受版本 | 1796 | 1792 |
| 其他来源观测区间内接受的主版本 | 10 | 11 |
| 与提交重叠的其他来源数 | 2 | 2 |
| 已观测匹配版本FPS | 59.701 | 56.067 |
| 相邻QPC系统同步间隔P95 ms | 20.838 | 27.784 |

日志 target/annotation-source-wait-{related,native1,native2}.log。capture及呈现记录丢弃、主来源混合与未匹配提交均0。这些反例证明当前受控路径没有“必须观察到全部来源比较完成才能提交”的全局CPU确认门槛；Pending→Confirm只表示CPU观察状态，GPU可能已完成而尚未轮询，不能解释为GPU实际忙碌区间、物理显示或无队列依赖。未测首次采集、实际App竞争和DWM取批内部。两轮差异不是性能优化，第二轮低于57FPS目标，第四次刷新仍未定位。

下一GPU范围聚焦主候选自身的GPU完成/CPU确认和提交后合成边界，保留上游分类；不要继续假设全来源确认门槛并盲目拆设备/线程。无Core接口、配置或依赖变化，AGENTS.md/DAG无冲突；未暂存、提交或推送。

最终门禁83360 exit0：workspace563通过/71忽略；Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release及diff check通过。日志 target/annotation-source-wait-final-{workspace,clippy,release,diff}.log。生产代码未改，Release门禁复用已编译产物；仅测试原生夹具前已完成Release测试编译。新增离线模块和报告保留，完整App/IME/DPI/资源/物理显示与GPU额外刷新归因仍未完成，整体目标继续进行。

## 2026-10-04：保留全部来源的主来源优先处理（无稳定收益，已撤回）

确认没有全来源CPU确认门槛后，单独对照来源处理顺序。微软[Immediate Context线程约束](https://learn.microsoft.com/en-us/windows/win32/direct3d11/overviews-direct3d-11-render-multi-thread-intro)要求序列化context及相应DXGI操作；再次核查[OBS WGC实现](https://github.com/obsproject/obs-studio/blob/master/libobs-winrt/winrt-capture.cpp)，不能从其回调取帧推导共享GPU无锁并发。已有带查询的主候选copy/compare/readback跨度较短，但GPU绝对排队位置仍未测出；不相减跨轮P95来宣称等待时间。

本批只在cfg(test)改变sample_gpu遍历：主HWND先处理，其他来源保持原相对顺序；全部来源、帧池、比较/确认、唤醒、效果及提交策略不变。图层用原枚举索引恢复桌面层序，窗口签名及变化累计仍沿原规则。基线与实验使用同一测试遍历/恢复实现，区别只有优先HWND；代码不预设生产优先级策略。三个冻结模式（全部原序、全部主优先、仅主参考）的Blur/Pixelate各3,686,400字节逐字节一致，并校验六处已知源色。

30930 exit0，像素前提通过后按基线→优先→基线→优先各5秒预热+30秒，60Hz源/约144Hz输出、完整覆盖ROI、已有composition/native等待探针，未并发构建。

| 指标 | 基线1 | 优先1 | 基线2 | 优先2 |
|---|---:|---:|---:|---:|
| 捕获来源数量 | 4 | 4 | 4 | 4 |
| worker调用毫秒/秒 | 161.220 | 148.436 | 140.130 | 145.383 |
| GPU完整处理P95 ms | 0.961 | 0.978 | 0.940 | 0.944 |
| 已观测匹配版本FPS | 59.534 | 53.467 | 51.400 | 54.032 |
| QPC更新间隔P95 ms | 20.838 | 34.729 | 34.729 | 34.729 |
| 超过3次刷新比例 | 0.013445 | 0.135371 | 0.174562 | 0.117284 |
| CPU比较开始→确认P95 ms | 2.1269 | 2.1051 | 2.2067 | 2.1534 |
| 取帧→接受提交P95 ms | 4.587 | 4.503 | 4.577 | 4.678 |
| 发布→OS同步上界P95 ms | 16.2624 | 17.2211 | 17.1753 | 17.1158 |

capture/呈现记录丢弃、混合/未匹配主版本均0。主优先未稳定改善整体：第一组FPS及刷新尾部恶化，第二组改善但仍低于57；两个优先组QPC间隔均34.729ms。相位/环境差异明显，不把次序当唯一根因，也不将略短CPU比较观察称为GPU队列改善。发布→同步上界不含实际UI或物理显示；阶段P95不相加，观测FPS是匹配下界。

日志 target/annotation-source-order-{pixels,baseline1,first1,baseline2,first2}.log。四份实验文件和当次基线存 target/annotation-source-order-experiment-20261004/{experiment,baseline}；按字节快照恢复并逐文件SHA256核对。实验字段、worker接线、枚举模式、报告及专用入口全部撤回，源码扫描无标识残留；前批独立提交离线诊断保留。下一步需要主候选GPU绝对完成/合成取批证据，或继续计划中的实际交互/资源审计，不继续重复该顺序实验。AGENTS.md与DAG兼容，无需修改；未暂存、提交或推送。

撤回后最终门禁77400 exit0：workspace563通过/71忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release及diff check通过。日志 target/annotation-source-order-final-{workspace,clippy,release,diff}.log；恢复原源码后门禁复用已有编译产物，不将实验状态测试当作发布源码证据。第四次刷新归因及完整App/IME/DPI/资源/物理显示验收仍未完成，整体目标继续进行。

## 2026-10-04：保留Port清理/重开资源验证与完整退休崩溃

继续交接及资源审计。参数入口现有会话/面板/目标校验及close_panel拖动清理未发现新增缺陷；转向生命周期实际缺口：旧gpu_backend_reopen测试每轮drop整个Port，未覆盖App保留同一Port、clear之后重开。新增仅测试performance_lifecycle模块，复用进程句柄/GDI/USER计数，12轮交替场景每次等待同stamp成功呈现，clear后等待worker局部scene的Weak失效、旧completion为空，再用固定250ms观察WGC异步资源退休。250ms只是计数观察窗口，不是生产卸载修复或安全保证。

89218第一项Release原生测试通过（5.90s）：12轮clear后的(handle,GDI,USER)全部(329,8,8)，最终drop Port后(309,8,5)，fixture释放后(301,5,2)。该证据支持本机受控同Port清理过程无观察到的这些资源增长，不覆盖PrivateBytes/GPU驱动内部、完整App/IME/双屏，不能作为全面资源验收。

同一串行命令第二项旧drop/reopen测试以0xc0000005崩溃，89218 exit1；后续workspace/Clippy/Release门禁停止。Windows Application Error记录模块GraphicsCapture.dll_unloaded（10.0.26100.9549）、偏移0x105ac。本机minidump只提取异常/代码指针进行分析：异常参数类型8（执行访问冲突），RIP位于已卸载模块0x105ac，少量栈代码指针落kernel32线程启动路径；不能据此猜测具体内部函数或完整线程因果。转储未复制进仓库，选择性诊断日志 target/annotation-retained-port-lifecycle-crash-triage.log，临时解析器位于target，仅输出指针/模块而不输出业务数据。

为排除新helper导致缺陷，用77400前的字节基线performance.rs执行相同旧测试：第一进程10轮通过，第二新进程同样0xc0000005失败，说明可在本批改动前代码复现且具有时序不稳定性；每次控制命令finally恢复当前文件并核验SHA256。日志 target/annotation-retained-port-lifecycle-{native,drop,baseline,baseline2}.log。未把控制脚本自身exit0误记为原生测试通过。

检索找到FFmpeg作者在[Win32CaptureSample #99](https://github.com/robmikh/Win32CaptureSample/issues/99)报告Close后RoUninitialize/FreeLibrary崩溃和疑似内部后台工作未退出；[FFmpeg当前实现](https://github.com/FFmpeg/FFmpeg/blob/master/libavfilter/vsrc_gfxcapture_winrt.cpp)持有GraphicsCapture模块句柄以绕开RoUninitialize卸载。该报告与本机失效形式相似，尚不足以证明同一内部根因。先前局部工厂修复继续保留；本次是已卸载代码执行，不能未经证据归为同一个静态工厂缓存调用。

下一P0先审计最终apartment退休、异步WGC关闭和代码模块所有权，建立可验证的退出协议；不盲目添加100ms sleep、无限固定DLL或只给测试持有额外apartment来掩盖失败。当前新增资源回归保留，生产逻辑未改；本批没有完整门禁通过声明。AGENTS.md/DAG仍兼容，无需修改；GPU第四次刷新、实际交互及资源完整验收仍未完成，无暂存、提交或推送。

## 2026-10-04：WGC代码映射与采集资源分开退休

针对旧字节基线可复现的GraphicsCapture.dll_unloaded执行访问冲突，审计现有销毁顺序：subscription先停并移除FrameArrived，session/pool Close并释放，Backend及显示资源在worker的apartment之前销毁，Port drop join worker。未找到项目自有对象越过apartment的证据。微软[FramePool.Close](https://learn.microsoft.com/en-us/uwp/api/windows.graphics.capture.direct3d11captureframepool.close?view=winrt-26100)公开释放资源，没有暴露内部后台线程退出完成信号；FFmpeg问题记录及其显式模块引用支持检查代码卸载竞态，但不提供已验证的全内部join接口。

基于实际已卸载代码执行与不可观察的内部退出边界，调整前批“不盲目固定DLL”的策略：Platform内部screen_capture::runtime仅在首次WGC能力使用时从System32加载GraphicsCapture.dll，并以实际模块地址调用GetModuleHandleExW(FROM_ADDRESS|PIN)。按[微软PIN语义](https://learn.microsoft.com/en-us/windows/win32/api/libloaderapi/nf-libloaderapi-getmodulehandleexw)，映射保持到进程终止；不把最后一个apartment退休视为可安全卸载该代码的证据。OnceLock只执行一次，临时LoadLibrary引用无论成功或失败均释放；能力查询、直接Backend及pool准备入口共同遵守此边界。加载/保留失败沿原不可用/错误路径退出。局部factory机制仍保留，未恢复生成的静态工厂缓存。

此为有代价的系统卸载竞态规避，不声称修复Windows内部线程退出：DLL映射及其全局状态留到进程结束，采集session、pool、回调、纹理、GPU设备、原生窗口及worker仍按所属实例释放。没有睡眠、隐藏窗口、常驻GPU会话、新Core接口/配置/依赖。AGENTS.md要求实例资源销毁，与平台代码映射的独立寿命兼容，无需修改规则；代码映射不能用工作集修剪替代。

88072 exit0：screen_capture相关16通过/8忽略；新增显式Release原生映射测试在16个独立线程的完整apartment退休后仍保持同一模块身份。GetModuleInformation实测SizeOfImage=262144（256KiB），这是映射大小，不是PrivateBytes、工作集或GPU内存。8个独立Release进程各10轮完整Port销毁/重开，共80轮全部通过（每进程约5.2秒），未增加测试apartment或修复睡眠；每轮退休handle/GDI/USER均(302,8,5)，fixture释放后(302,5,2)。此前成功基线为301个句柄，修复观测到固定多1个句柄，不能声称零常驻成本；没有逐轮增长。

同Port clear/reopen12轮继续全部(329,8,8)，最终drop Port(310,8,5)，fixture释放(302,5,2)，Weak及旧completion检查通过（5.78秒）。随后旧factory完整退休8轮通过，Blur/Pixelate冻结输出各3,686,400字节最大通道差0。日志 target/annotation-wgc-code-lifetime-{related,mapping,reopen1,reopen2,reopen3,reopen4,reopen5,reopen6,reopen7,reopen8,clear,factories,pixels}.log。这些证据验证当前机器退出稳定性及观察资源，不证明所有OS版本/驱动、完整App/IME/跨屏或物理显示；没有重测GPU吞吐，不能当第四次刷新的性能修复。

首轮55336 workspace通过，Clippy拒绝新增retained-Port测试的常量assert，Release未运行；改为既有测试相同的if cfg!(debug_assertions) panic保护，未改变Release行为或生产路径。最终39884 exit0：workspace563通过/73忽略，Clippy --workspace --all-targets -- -D warnings、UI现有build.rs/Slint集成、Windows workspace Release（1m06s）及diff check通过。日志 target/annotation-wgc-code-lifetime-final-{workspace-fixed,clippy-fixed,release-fixed,diff-fixed}.log。显式执行的ignored原生回归与默认workspace忽略项分别记录；不把73个ignored算通过。整体GPU额外刷新、实际App/IME/跨屏/PrivateBytes/物理显示要求仍未完成，无暂存、提交或推送。

## 2026-10-04：同Port清理/重开的进程内存与线程观测

前一目标轮只回答依赖审计问题，没有修改权威状态；本轮补齐批次5资源证据。performance_lifecycle现有ignored原生夹具从12轮延长至64轮，每轮确认同stamp呈现、clear后scene Weak退休及旧completion清空，再按既有250ms观察窗口读取GetProcessMemoryInfo。只在测试清理阶段记录PrivateUsage/WorkingSetSize，不新增生产热路径查询、日志、线程或公开接口。PrivateUsage是进程私有提交量，WorkingSetSize是当前驻留页，不能互相替代，官方定义见 https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters_ex 。没有添加任意内存上限或把分配器留存当作精确泄漏证明。

43789 Release测试编译通过（35.70秒），三个独立进程各12轮均通过现有句柄/Weak/窗口检查；短循环显示工作集上升，故增加64轮。94864新Release编译通过（36.23秒）；8230两独立进程各64轮，单进程30.87/30.81秒，均通过已有所有权和句柄断言。每进程62轮handle/GDI/USER为(324,8,8)、2轮为(323,8,8)，最终Port释放(304,8,5)、fixture释放(296,5,2)。与历史句柄总数不同是不同进程/环境，使用本轮趋势，不将跨进程绝对数差宣称泄漏。

| 清理后16轮区间 | 第1进程PrivateBytes中位数MiB | 第2进程PrivateBytes中位数MiB | 第1工作集中位数MiB | 第2工作集中位数MiB |
|---|---:|---:|---:|---:|
| 0–15 | 124.436 | 129.896 | 40.664 | 40.266 |
| 16–31 | 127.633 | 132.076 | 43.852 | 43.045 |
| 32–47 | 130.756 | 132.438 | 46.615 | 45.371 |
| 48–63 | 138.660 | 138.826 | 52.514 | 50.576 |

这些数据没有证明内存稳定。最后16轮比首16轮的私有提交中位数增加14.224/8.930MiB，Port和fixture释放后仍分别约139.762/132.688MiB私有提交；不能将句柄断言通过当作内存验收通过，也不能据此直接归咎于DLL pin、WGC或驱动。分配器、模块全局缓存、驱动资源和真实对象保留尚未归因。下一步需分配来源/最小生命周期对照；不使用工作集修剪掩盖增长，不恢复有退出崩溃的卸载路径。

外部PowerShell以名义100ms（实际含进程查询开销）采样同一测试PID，267/275个样本；5秒之后线程数最常见12（149/159个样本），重建期曾达120/119。末尾采样均12；该采样不能归属线程、不能证明无未观测峰值，进程退出后不再采样。没有向生产加入线程枚举，不把短暂创建解释成持久线程泄漏，也不以全进程采样替代App线程生命周期。日志target/annotation-memory-lifecycle-native{1,2,3,4,5}.log、threads{1,2,3,4,5}.csv；前3为12轮，后2为最终64轮。

搜索OBS和Win32CaptureSample的相关公开问题，没有找到足以对应本次增长且已证实的修复；搜索中的QSV/NVENC编码器资源问题不属于本项目无编码路径，未套用其结论。未修改AGENTS.md、Core接口、依赖、配置、生产呈现策略或GPU等待。没有重测延迟，也没有额外刷新收益声明；实际App/IME/跨DPI/物理显示和内存归因仍未完成。

76517最终门禁exit0：Platform相关139通过/62忽略，workspace563通过/73忽略，Clippy --workspace --all-targets -- -D warnings、UI现有build.rs/Slint集成、Windows workspace Release（0.65秒复用生产产物）与diff check通过。显式ignored原生验证记录单列；现有资源断言通过只涵盖Weak/completion/句柄/GDI/USER，不涵盖本次发现的内存增长。日志target/annotation-memory-lifecycle-{build,build-long,related,workspace,clippy,release,diff}.log。未暂存、提交或推送；整体目标保持进行。

## 2026-10-05：内存增长的最小生命周期隔离

前轮已用64轮确认私有提交及工作集增长，句柄稳定不能作为内存通过。本轮新增5个ignored Release原生观测入口，共用observe_retirement及memory_bytes/counts，分别只构建Backend、构建空Renderer、创建1280×720双帧池但不StartCapture、实际采集而不构建Renderer/效果/呈现、只反复编译Graphics的10个shader但不创建GPU设备。每入口独立新进程64轮，沿用250ms观察窗口；没有新增生产计时/日志/线程/资源、Core接口、依赖或配置，没有添加臆测的内存稳定阈值。

60410/76140/16025 Release编译均通过（35.65/35.99/35.83秒）；30438四种初始化对照与61078纯编译及第二次空Renderer均exit0。初版每轮完整退出apartment，与生产同Port worker保留presentation_apartment不一致：空Renderer两个进程均有增长，纯shader编译基本稳定，但不把它们直接与生产相减。随后观察helper保留一个apartment贯穿64轮，匹配worker这条边界，结束后仍完整退休。它不是生产卸载修复，也不撤销此前DLL mapping pin或完整Port退休回归。

83792最终Release编译通过（35.94秒）；24293五个独立新进程的最终保留apartment对照全部exit0：Backend27.55秒、Renderer29.32秒、空帧池27.81秒、实际采集29.43秒、纯shader编译18.22秒，共320轮。下表为每16轮清理后PrivateBytes中位数MiB，独立进程之间不做数值相减来估算某个模块的精确分配：

| 对照 | 0–15 | 16–31 | 32–47 | 48–63 |
|---|---:|---:|---:|---:|
| Backend，无采集会话 | 20.045 | 21.342 | 21.617 | 21.914 |
| 空Renderer，无采集/效果/呈现 | 25.717 | 27.438 | 28.832 | 32.176 |
| 帧池，无StartCapture | 21.229 | 23.436 | 28.852 | 33.426 |
| 实际WGC，无Renderer/效果/呈现 | 90.012 | 92.738 | 94.207 | 95.830 |
| 10个shader编译，无GPU设备 | 2.260 | 2.586 | 2.586 | 2.590 |

工作集分段中位数分别25.584→28.713、29.291→39.281、26.969→36.920、39.230→45.129、11.910→12.254MiB。Backend后半段较平稳，shader编译后半段基本稳定；空Renderer及空帧池在没有实际采集时也增长，实际采集无Renderer仍增长。因此实际WGC采集不是进程内存增长的必要条件，不能把所有增长归为采集对象或DLL pin；纯编译数据也不支持只靠缓存shader字节码解决该增长。尚未归属具体heap/驱动/GPU资源，不能证明真正泄漏或分配器/驱动留存。下一资源行动需分配来源及GPU对象创建/退休归因，而非维持Renderer或隐藏窗口掩盖重开问题。

全部64轮的handle/GDI/USER对各入口分别固定(278,0,2)、(279,4,2)、(279,4,2)、(309,9,6)、(165,0,2)。实际采集每轮4个来源且确认owned源GPU纹理存在；不是单来源WGC隔离，也没有为归因省略底层窗口。GPU效果、输出、surface和render worker未参与实际采集对照；空Renderer包含Graphics shader对象和state创建，但没有绑定绘制/效果/呈现。这些观测不是实际App/IME/跨屏/物理显示证明，也不证明Driver内部所有资源释放。

再次核查 https://github.com/obsproject/obs-studio/blob/master/libobs-winrt/winrt-capture.cpp ：其正常free先撤销frame/closed事件，Close frame_pool，再Close session并delete capture；本项目先撤销事件、Close session再Close pool。这个差异没有对应本轮空Renderer和无session帧池增长的因果证据，不把交换关闭顺序当作已知修复，也没有引入OBS全局graphics锁或缓存存活策略。AGENTS.md与本轮仅测试隔离及现有模块边界兼容，无需修改。

日志target/annotation-memory-isolation-{backend_without_capture_sessions-1,renderer_without_capture_sessions-1,renderer_without_capture_sessions-2,frame_pool_without_capture_sessions-1,active_capture_without_renderer-1,shader_compiler-1}.log为初版完整apartment退休；最终日志target/annotation-memory-isolation-retained-{backend_without_capture_sessions,renderer_without_capture_sessions,frame_pool_without_capture_sessions,active_capture_without_renderer,shader_compiler_without_gpu_device}.log，统计表target/annotation-memory-isolation-retained-summary.csv（每入口确认64轮完整后生成）。统计不加入生产热路径，所有门禁和观测结果与性能收益分开。

8234最终源码门禁exit0：Platform相关139通过/67忽略，workspace563通过/78忽略，Clippy --workspace --all-targets -- -D warnings、UI现有build.rs/Slint集成、Windows workspace Release（0.80秒复用生产产物）及diff check通过；5个新ignored观测显式执行单列，不把内存增长视为通过。日志target/annotation-memory-isolation-{related,workspace,clippy,release,diff}.log。生产路径未改变，没有重测GPU额外刷新、没有延迟收益声明；内存归因、实际App/IME/DPI及物理显示验收继续，整体目标未完成。未暂存、提交或推送。

## 2026-10-05：默认堆活分配、虚拟页分类与退休Flush反证

前轮最小对照只能说明PrivateBytes增长，不足以区分空闲页留存与活分配。本轮将原process_bytes测量抽入仅测试performance_memory模块，保留原指标，新增默认堆HeapSummary及VirtualQuery地址范围分类。HeapSummary通过kernel32动态解析，可用性失败记录None，不将测试二进制最低Windows版本提升到该API的20348要求；只查询GetProcessHeap，不使用可能被第三方并发销毁的GetProcessHeaps句柄。VirtualQuery只记录MEM_COMMIT的PRIVATE/MAPPED/IMAGE字节及完整遍历状态，不读取页面内容/输出地址，不将PRIVATE范围与PrivateUsage混为同一指标；copy-on-write图像页仍属于IMAGE/MAPPED，类别不能相减推算精确所有权。官方语义见 https://learn.microsoft.com/en-us/windows/win32/api/heapapi/nf-heapapi-heapsummary 、 https://learn.microsoft.com/en-us/windows/win32/api/heapapi/nf-heapapi-getprocessheaps 、 https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualquery 。

98897 Release编译通过（35.69秒）；95832五个独立进程分别64轮Backend、空Renderer、未启动帧池、无Renderer的实际WGC及完整同Port clear/reopen全部完成（27.64/29.07/27.52/29.81/30.26秒）。每入口在0/15/31/47/63轮清理后读一次分类，所有VirtualQuery遍历complete=true、默认堆API均可用。Backend堆已分配量543034→560557字节基本平稳，已提交量5566464→9035776增长，至少这部分区别于活分配增长。其他入口不平稳：

| 对照 | 第0轮默认堆已分配bytes | 第15轮 | 第31轮 | 第47轮 | 第63轮 |
|---|---:|---:|---:|---:|---:|
| 空Renderer，无采集 | 625417 | 1547817 | 2528041 | 3508265 | 4488489 |
| 未StartCapture帧池 | 623457 | 1547585 | 2527857 | 3508385 | 4488609 |
| 实际WGC，无Renderer | 897620 | 1964774 | 2995110 | 4015536 | 4991588 |
| 完整同Port clear/reopen | 921754 | 2029286 | 3005834 | 4011986 | 4691098 |

两个无实际采集对照的后几个16轮区间均接近增加980224字节，即每轮61264字节（约59.83KiB）。不能再把增长全部解释为工作集、默认堆空闲页或shader编译；默认堆确实出现仍被分配的块。不过活分配仍可能属于库/驱动缓存，尚未证明具体对象泄漏或分配栈。相同进程的MAPPED/IMAGE范围字节在本轮各快照保持不变，PRIVATE范围增长；不是每轮重新增加代码映像的证据，不能直接归罪于GraphicsCapture代码映射pin。与PrivateUsage数据不做不合法的跨指标减法。

进一步只在0和63轮记录默认堆busy块大小直方图：使用HeapLock/HeapWalk，锁内固定256槽数组、最多65536个条目，不分配/格式化/记录内容；在解锁后输出大小/数量。只查询本进程拥有的默认堆，所有者不明的private heap不遍历。使用原生BOOL ABI避免锁内Error格式化分配，SDK PROCESS_HEAP_ENTRY_BUSY=0x0004有明确注释；256类别/遍历上限、API失败均标记未完成，不假装完整统计。这个锁会影响后台线程，因此仅用于已退休资源夹具，绝不能作为延迟/吞吐数据或移入生产。HeapWalk官方调试用途及锁影响见 https://learn.microsoft.com/en-us/windows/win32/api/heapapi/nf-heapapi-heapwalk 。

64233 Release编译通过（36.73秒）；50995两个独立进程各64轮空Renderer/未启动帧池完成（29.03/27.74秒），两个首尾直方图均available/complete=true。差分各增加1071个3600字节块及63个64字节块：1071=17×63，主要大小组净增3855600字节；每次重建的共同签名为17×3600+64=61264。还有少量其他类别波动，不把签名当作具体类型或归属的证明。两个对照仍没有业务轨迹、正文缓存、GPU效果、呈现或实际WGC会话；疑点收窄到相关GPU资源初始化/退休涉及的运行时或后台分配，尚需栈/线程来源证据。

按微软 https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-flush 的延迟销毁说明，做仅测试单因素反证：临时保留context引用，先释放整个Renderer或pool/backend的其他引用，再ClearState/Flush，之后释放context。未加入GPU完成等待，也未修改生产Drop。29409实验Release编译通过（36.33秒）；24588两个各64轮新进程完成（29.18/27.39秒），两者仍增加相同1071个3600字节块及63个64字节块；默认堆后段增长同样约61264/轮。没有证据支持以此Flush修复增长，两个专用入口和布尔实验接线已全部删除并扫描无残留；实验源码存target/annotation-allocation-flush-experiment，日志保留。没有在生产热路径、关闭路径或设备恢复路径增加Flush。

困难排查继续参考公开实现与问题：搜索D3D11CreateDevice/线程/3600字节/泄漏，未找到匹配本轮签名且已证实的开源修复。检索中的D3D11/D3D12共享句柄重复包装问题不属于本项目当前路径，没有套用结论。下一步查固定17个块的线程/运行时归属或分配栈，不以猜测释放内存、不维持旧GPU设备或隐藏窗口、不用工作集修剪掩盖增长。AGENTS.md及七crate DAG兼容，无需修改；没有新增Core接口、配置、依赖或生产资源。

60798最终门禁exit0：Platform相关139通过/67忽略、workspace563通过/78忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成、Windows workspace Release（0.75秒复用生产产物）及diff check通过。显式原生分类320轮、直方图128轮与失败Flush实验128轮分开记录；通过指API路径/既有所有权断言完成，不意味着内存验收通过。日志target/annotation-allocation-categories-{build,related,workspace,clippy,release,diff}.log、annotation-allocation-categories-<test>.log、annotation-allocation-histogram-<test>.log及差分csv、annotation-allocation-flush-<test>.log。未重测GPU刷新/实际App/IME/DPI/物理显示，无延迟收益声明；整体目标继续，未暂存、提交或推送。

## 2026-10-05：原生D3D纹理销毁通知与设备边界隔离

继续批次5资源归因；上一答复只核对GPU等待证据，没有生产修复。本批先复核前次已完成但未入账的线程实验。临时测试D3D11_CREATE_DEVICE_PREVENT_INTERNAL_THREADING_OPTIMIZATIONS，硬件为NVIDIA GeForce RTX 4060 Laptop GPU，16物理核心/32逻辑处理器。89330 Release编译通过；39849基线→限制线程→基线→限制线程→限制线程帧池，五个独立进程各64轮全部完成。外部名义100ms线程采样分别256/258/263/259/247个样本，观测峰值75/42/75/42/42；这是采样峰值而非所有瞬时线程。五组堆签名均仍新增1071个3600字节块和63个64字节块，没有消除每轮17×3600+64=61264字节增长。此标志没有延迟测量或生产收益证据；临时构造标志、两个入口及接线均撤回，capture.rs按字节快照恢复并核对SHA256。实验存target/annotation-driver-thread-isolation-experiment，日志annotation-driver-thread-*.log及*-threads.csv。没有加入生产线程限制。

新增仅测试原生D3D11设备与纹理对照，不使用Backend/WinRT包装、WGC、shader编译、Renderer、效果或呈现。98721 Release编译通过；88276两个独立64轮过程完成。只创建设备默认堆已分配533407→551586字节基本平稳；设备加一张1280×720 RGBA DEFAULT、shader-resource/render-target纹理613264→4479072字节，仍出现1071×3600及63×64净增。实际采集或本项目渲染器不是此签名的必要条件，尚不能据此归属驱动具体分配栈。

困难排查找到MeldStudio/amd-vram-leak（https://github.com/MeldStudio/amd-vram-leak）的原生销毁通知诊断方法。其AMD、多设备共享纹理、VRAM和Factory Flush修复条件与本项目NVIDIA单设备非共享默认堆增长不同，未套用其修复。按微软ID3DDestructionNotifier语义（https://learn.microsoft.com/en-us/windows/win32/api/d3dcommon/nn-d3dcommon-id3ddestructionotifier）接入原生销毁通知：callback只持有Arc原子计数，不保留COM对象，不访问被销毁对象；注册失败回收计数引用，接口不可用明确计入未注册而非伪造成功。此探针只在ignored资源夹具中运行，不进入生产或延迟测量。

71151 Release编译通过41.36s；45053两个独立64轮对照完成27.12/27.39s。设备通知注册/收到(64,0)/(64,0)，纹理对照(64,64)/(64,64)，全部收到。纹理默认堆622288→4487792字节，3600字节块17→1088、64字节块100→163，签名不变。不能再仅凭Rust Drop推断销毁，现有证据是原生对象销毁通知；仍不证明驱动内部/显存全部释放。

新增同设备连续创建/销毁64张纹理对照。首版在MTA初始化前创建设备，条件不同，72714虽完成但只保留原始日志，不作同条件归因。修正为先初始化MTA，19125 Release编译38.91s及原生16.68s全部完成。设备/纹理注册与收到均(1,64)。默认堆已分配4752103→4924827字节，64字节块1201→1201，无3600字节类别，未出现每轮重建设备的固定签名；释放设备250ms观察后默认堆622646字节。但MEM_COMMIT PRIVATE范围22491136→286019584字节，释放后246239232字节，明显未稳定。对象销毁通知与内存稳定必须分开判断；同设备既不是已验证修复，也不能成为长期保留设备或隐藏窗口的理由。后续需查设备创建首次资源初始化的分配来源，以及同设备资源退休后的私有提交/后台页留存，不能手动释放未知块或用工作集修剪掩盖增长。

最终对照日志target/annotation-destruction-lifecycle_memory_raw_{device,texture}_without_winrt.log、annotation-destruction-retained-mta-{build,native}.log；首版保留annotation-destruction-retained-{build-fixed,native}.log。没有重测27.783ms、实际App/IME/双屏/物理显示；本批资源证据不能充当OS额外刷新根因。新增三个ignored原生入口和计数接线，无Core公开接口/配置/依赖/生产策略变化，AGENTS.md及DAG兼容，不暂存、提交或推送。整体资源和性能目标仍未完成。

61517最终源码门禁exit0：annotation_gpu相关54通过/45忽略、workspace563通过/81忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成、Windows workspace Release（0.70s复用生产产物）及git diff --check通过。日志target/annotation-destruction-final-fixed-{related,workspace,clippy,release,diff}.log。ignored资源夹具显式执行单列，不把观测完成算作内存通过。早一轮82196门禁也通过，但其后MTA初始化顺序修正，以61517为最终源码依据。

## 2026-10-05：同设备纹理的延迟退休对照

上一批原生销毁通知已证明设备/纹理回调到达，但同设备连续创建纹理仍有PRIVATE提交范围增长。本批只在ignored资源夹具增加ClearState/Flush干预，普通入口和实验入口共用observe_retained_raw_device；先释放纹理，再清除状态并Flush，保留设备至64轮结束。没有绘制、WGC、效果、共享纹理、呈现或GPU完成等待；Flush返回不能当作GPU完成信号。微软 https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-flush 说明D3D延迟销毁、ClearState/Flush的用途及性能代价。本次条件与此前每轮重建设备的退休Flush反证不同，不重复声称能修复所有增长。

32555 Release编译40.31s后基线/干预各64轮完成16.72s；8098第二组独立基线/干预各64轮完成16.80/16.67s，未并发构建。四进程均设备/纹理销毁注册和收到(1,64)，全部VirtualQuery遍历complete=true。干预只改ClearState/Flush组合，没有区分两者各自贡献。结果为MEM_COMMIT PRIVATE范围字节，非工作集或显存：

| 对照 | 第0轮 | 第15轮 | 第63轮 | 最后释放设备250ms观察后 |
|---|---:|---:|---:|---:|
| 基线1 | 21991424 | 85561344 | 291143680 | 254791680 |
| ClearState/Flush1 | 23556096 | 24604672 | 24604672 | 18341888 |
| 基线2 | 22953984 | 88621056 | 302604288 | 261447680 |
| ClearState/Flush2 | 22126592 | 23699456 | 23699456 | 17371136 |

两个干预进程从15至63轮PRIVATE范围及默认堆已分配均不再增长，基线继续增长。基线默认堆已分配4752711→4925435及4753671→4926395；干预4762030→4761246及4760510→4759726，最终释放设备分别623179/621659。支持同设备对照的增长可被延迟退休干预消除，仍未识别具体后台分配栈，不证明所有驱动缓存/显存释放。原先重建设备的17×3600+64签名依然有独立反证，不能用本结果宣称解决。

仅测试共用对照和新ignored入口保留。没有向生产每帧、每对象Drop或关闭流程增加Flush，没有保留跨会话设备/隐藏窗口；现有生产clear会销毁Renderer，恢复期间保留旧表面具有不同所有权条件。下一步验证真实同Port清理边界是否具有对应未退休资源及干预作用，避免把同设备夹具收益直接推广；重建设备堆分配来源仍待查。AGENTS.md/DAG兼容，不新增Core接口/配置/依赖，不暂存、提交或推送。没有重测GPU吞吐、27.783ms或物理显示，性能及完整App/IME/DPI验收仍未完成。

日志target/annotation-retained-flush-build.log、annotation-retained-flush-[repeat-]<test>.log；实验起点target/annotation-retained-flush-experiment/baseline.rs。两组实验用相同可执行文件，最终仅更正日志label及解释注释，最终源码门禁另列。

93062最终源码门禁exit0：annotation_gpu相关54通过/46忽略、workspace563通过/82忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成、Windows workspace Release（0.78s复用生产产物）及git diff --check通过。日志target/annotation-retained-flush-final-{related,workspace,clippy,release,diff}.log。显式四个64轮原生过程另列，不把ignored或资源局部稳定当完整验收。整体目标保持进行。

## 2026-10-05：真实Port清理边界的退休Flush反证

上一轮同设备纹理释放的ClearState/Flush干预保持PRIVATE范围稳定，但生产Port清理会释放整个Renderer/Backend及旧表面，不能直接套用。本批先保存annotation_gpu.rs与performance_lifecycle.rs字节快照，临时增加cfg(test) Mailbox退休干预标志及计数，仅在generation变化清理分支保留context引用，释放Renderer与retained_surfaces后，在Mailbox锁外ClearState/Flush，释放context并累计计数。不是每帧Flush、GPU完成等待或生产销毁策略。完整Port夹具共用同一函数、默认不干预，实验额外入口开启标志；最终断言64次干预确实执行，基线0次。

首轮编译因测试误写Renderer.backend失败（该字段实际为capture），修正后28131 Release编译40.35s通过；基线及干预独立新进程各64轮完整提交/clear/重开完成30.58/30.68s，均通过同stamp成功呈现、scene Weak释放、旧completion清空、handle/GDI/USER及fixture资源断言。全部页分类complete=true。默认堆活分配如下：

| 对照 | 第0轮 | 第15轮 | 第31轮 | 第47轮 | 第63轮 |
|---|---:|---:|---:|---:|---:|
| 原清理 | 941998 | 2050298 | 3037940 | 4039920 | 4756580 |
| 清理后ClearState/Flush | 925103 | 2025203 | 3022563 | 4012027 | 4753719 |

两组3600字节块均18→1089，净增1071；64字节块均139→201，净增62，其余大小组有波动。固定增长没有减少。PRIVATE范围基线145121280→143015936、干预119255040→138465280，包含较大中间波动，不能从首尾或跨进程数值估计精确节省。当前实验不支持把同设备纹理的退休收益推广到真实Port关闭；也不证明所有延迟退休已被观察或Flush绝不有用。未重测GPU吞吐、OS刷新或物理显示，资源测试不能作为27.783ms根因。

临时标志/计数、context接线、共享helper和专用入口全部按字节快照撤回，两个文件SHA256均恢复一致；原同设备诊断入口保留。实验源码target/annotation-port-retirement-experiment/{baseline,experiment}，日志target/annotation-port-retirement-{build,build-fixed,gpu_clear_reopen_retained_port_releases_scene_and_window_resources,gpu_clear_reopen_port_with_retirement_flush}.log。没有向生产加入Flush、隐藏窗口、保留GPU设备或工作集修剪。下一资源行动应查设备首次资源初始化产生的固定堆块来源，避免重复已经失败的退休Flush策略；完整App/IME/DPI/物理显示及GPU第四次刷新仍待验收。AGENTS.md及DAG无冲突，不新增Core接口/配置/依赖，不暂存、提交或推送。

82825恢复后的最终源码门禁exit0：annotation_gpu相关54通过/46忽略、workspace563通过/82忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成、Windows workspace Release（0.64s复用生产产物）及git diff --check通过。日志target/annotation-port-retirement-final-{related,workspace,clippy,release,diff}.log。扫描源码无retirement_flush/observe_port_retirement实验标识；原生128轮反证单独记录，未将观测完成或撤回门禁通过当资源验收通过，整体目标继续。

## 2026-10-05：空线程与驱动驻留的独立分配签名

上一轮真实Port退休Flush没有减少3600字节固定块，本轮按开源最小对照思路检查线程attach/detach。参考 https://github.com/ArtistDeveloper/nvwgf2umx-thread-leak ：作者在不同硬件/OS/驱动报告nvwgf2umx每线程184字节，并给出分配栈。其签名与本项目17×3600不同，不预设同一根因，也没有执行仓库要求提升权限的IFEO/UST注册表修改或安装调试工具。

新增两个ignored Release资源入口：相同MTA及observe_retirement，64轮每轮17个逐个创建、正常join退出的空线程；控制不初始化GPU，另一入口先创建并保留一个原生device/context及一张1280×720纹理，64轮期间不新增纹理、D3D调用、WGC、shader、效果、呈现或COM线程初始化。驱动GetModuleHandleW开始/结束记录control=false/false、GPU=true/true。后者还有D3D运行时/设备初始化差异，不能仅凭该对照把分配栈归为nvwgf2umx。

98880 Release编译38.72s及两个过程16.35/16.64s均完成：控制默认堆237132字节全程不变，GPU4757699→4960539。但首尾GPU直方图available=true、complete=false，256个类别容量不足；初次不完整结果不能推导184/3600类缺失。随后仅测试performance_memory容量改为512个固定栈槽（约4KiB），锁内仍不分配/格式化/日志，容量/65536遍历上限和API失败仍标记不完整。此为解决已观察诊断截断，不移入生产热路径。

81529最终Release编译38.78s；两个新进程各64轮完成16.35/16.65s，首尾直方图和VirtualQuery均complete=true。控制默认堆237132全程不变；GPU默认堆4756674→4958602。后63轮额外创建1071=63×17个空线程，GPU直方图净增1071个184字节块（197064字节）及16个304字节块，控制无类别净增；3600字节类别两次完整直方图均不存在，64字节类数量不变。GPU PRIVATE范围22351872→22654976，控制614400不变。该证据把空线程自身的184字节增长与每轮重建设备的3600字节签名分开；与公开报告相似但未有本机分配栈，不能声称已证明同一驱动缺陷。也不意味着GPU设备重建17个3600块只是线程attach副作用。

两个诊断入口及512槽探针保留，仅测试。初次日志target/annotation-empty-thread-{build,<test>}.log，完整重跑annotation-empty-thread-complete-{build,<test>}.log；没有生产线程池/设备驻留/驱动DLL pin策略变化，没有重测27.783ms、吞吐或物理显示。下一资源行动需要固定3600块的分配来源证据，不能据184签名把所有增长归咎驱动或重复无收益Flush。完整App/IME/DPI/资源和GPU额外刷新仍未验收；AGENTS.md及DAG无冲突，无Core接口/配置/依赖变化，不暂存、提交或推送。

70616最终源码门禁exit0：annotation_gpu相关54通过/48忽略、workspace563通过/84忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成、Windows workspace Release（0.62s复用生产产物）与git diff --check通过。日志target/annotation-empty-thread-complete-final-{related,workspace,clippy,release,diff}.log；33847为扩大探针前的门禁，不替代最终证据。四个64轮资源进程单列，完整直方图结果不等于资源验收通过，整体目标保持进行。

## 2026-10-05：WARP与硬件资源退休对照

前轮空线程184字节签名未复现设备重建的3600块，本轮按微软WARP驱动隔离用途（https://learn.microsoft.com/en-us/windows/win32/direct3darticles/directx-warp）增加仅测试WARP设备及设备+纹理退休入口。共用原生设备、纹理、销毁通知及64轮观察流程，只将D3D11CreateDevice的driver type改为D3D_DRIVER_TYPE_WARP；BGRA创建标志、默认feature level、1280×720 RGBA DEFAULT纹理、shader-resource/render-target绑定、MTA和250ms退休观察窗口不变。没有WGC、shader编译、Renderer、效果、呈现或绘制。WARP不初始化硬件D3D设备，但未采集完整模块加载跟踪，不能声称进程没有任何硬件相关模块；它是行为隔离对照，不是产品CPU渲染替代。

9785 Release编译38.69s后，WARP设备64轮16.64s、WARP纹理64轮16.43s、硬件纹理64轮26.54s全部完成。WARP设备销毁通知注册/收到(64,0)，WARP/硬件纹理(64,64)，全部匹配。默认堆活分配如下：

| 对照 | 第0轮 | 第15轮 | 第31轮 | 第47轮 | 第63轮 |
|---|---:|---:|---:|---:|---:|
| WARP设备 | 285597 | 285597 | 285597 | 285597 | 285597 |
| WARP设备+纹理 | 289413 | 289413 | 289413 | 289413 | 289413 |
| 硬件设备+纹理（首尾） | 619284 | — | — | — | 4483716 |

首尾直方图及VirtualQuery均complete=true。两个WARP入口均无3600字节类别，64字节块均49→49；硬件仍3600字节块17→1088（1071净增）、64字节块100→163（63净增）。WARP纹理PRIVATE范围4501504→4485120，硬件16961536→35246080。不能跨进程数值相减归属精确驱动成本。该结果将固定增长收窄到硬件设备路径，而非所有D3D11设备通用行为；仍缺具体分配栈，不能据此认定NVIDIA哪一函数或全部硬件必现。

随后补上实际适配器身份校验：仅WARP helper将device转换为IDXGIDevice，GetAdapter再读IDXGIAdapter1::GetDesc1，每次断言DXGI_ADAPTER_FLAG_SOFTWARE存在。22296最终Release编译39.13s，带身份断言WARP纹理64轮16.43s通过，注册/收到(64,64)，默认堆293585字节五个快照全相同，PRIVATE范围4460544→4485120；首尾直方图complete=true。新增查询解释了与前轮绝对堆值小差异，不将跨版本绝对差当收益；核实路径后仍稳定。

两个ignored入口、共用driver选择和软件身份断言仅测试保留，不改生产硬件默认或软件回退策略。日志target/annotation-warp-retirement-{build,<test>,identity-build,identity-native}.log。下一资源步骤需要硬件首次资源初始化固定块的分配来源/适配器对照证据，不重复已失败的Flush/线程限制实验；GPU额外刷新、完整App/IME/DPI/资源和物理显示仍未验收。AGENTS.md/DAG兼容，无Core公开接口/配置/依赖变化，不暂存、提交或推送。

91543最终源码门禁exit0：annotation_gpu相关54通过/50忽略、workspace563通过/86忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint集成、Windows workspace Release及git diff --check通过。日志target/annotation-warp-retirement-final-{related,workspace,clippy,release,diff}.log。显式四个64轮资源过程单列，不把ignored或WARP局部稳定当资源全面验收；整体目标继续。

## 2026-10-05：实际App说明输入与马赛克切换重入

上一轮真实Release App（owned PID17084）在125%主屏点击序号A，输入ab，选蓝色后继续输入c，原编辑HWND1836694保持；左移两次、字号加一后输入X，截图为aXbc，插入位置保留。100%副屏创建序号2、参数面板保持。证据target/annotation-real-app-caption-{before-color,after-color,after-size}.png及secondary-sequence.png。Unicode注入不验证中文IME，双屏无负坐标，不代替物理显示延迟。

切换马赛克后截图底部裁剪，但随后核查日志确认App已崩溃：17:36:34.951427Z annotation.rs:4875 RefCell already mutably borrowed；Windows Application事件1000进程0x42BC（17084）、异常0xc0000409。因此不能把旧截图直接当独立布局根因，也不能宣称此前App正常退出。软件像素回归mosaic_row_has_equal_vertical_margins_at_native_panel_sizes在100%/125%均通过，当前未修改Slint布局。

pointer_panel在读取会话前检测借用重入；若会话正被更新，将同一事件交给该窗口InputWork下一轮处理，继续使用会话/窗口实例校验；退休会取消排队消息。正常事件保持直接分发，不改平台通用机制。新回归native_panel_pointer_reentry_is_deferred_and_retirement_cancels_replay验证持有可变会话借用时的按下/松开不会panic、旧实例排队事件安全拒绝和退休取消；不替代真实App有效实例操作。相关单测通过，annotation UI 226通过/9忽略；workspace/Clippy/Release与实机复验尚在进行。没有GPU延迟收益声明，无暂存、提交或推送。

最终门禁58028 exit0：UI annotation226通过/9忽略，workspace565通过/86忽略，Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release（2m17s）及diff check通过；日志target/annotation-panel-reentry-{ui,workspace,clippy,release,diff}.log。新增两个UI回归，不把软件渲染结果当实际App验收。

新Release真实App owned PID24920在125%屏展开马赛克，窗口775×73物理像素。截图target/annotation-panel-reentry-mosaic-fixed.png已查看：整行居中，控件与底边间有留白，无裁剪；未修改布局。随后序号/马赛克连续切换8轮，进程Responding=True、窗口清单保存target/annotation-panel-reentry-switch-windows.txt；点击完成后参数/画布/主工具条均消失，后台进程仍在。最后仅停止本次启动且路径校验一致的smoke进程；不把Stop-Process当正常应用退出验证。此前截图为崩溃中的画面，不能作为独立布局缺陷。本轮实际面板只验125%，100%软件像素通过但不等同100%原生面板；IME、负坐标、完整资源和GPU额外刷新仍未完成。

## 2026-10-05：采集查询可读后是否仍被CPU确认拖延

针对关键路径连带等待假设，复用现有test-only GpuComparison与Confirm事件，按完整Published(token,sequence,ticks)配对。仅保留同主来源、唯一查询观察和唯一changed确认，查询观察须在comparison started至确认之间；重复、跨来源、旧生命周期、查询晚于确认均排除。新增纯回归覆盖这些拒绝条件。报告source_confirmation_gap明确为CPU观察到查询结果可读→CPU确认的间隔，不是GPU实际完成时间、GPU队列等待、WGC到达或OS同步。没有增加生产查询、轮询、线程或同步调用。

新增ignored入口gpu_live_capture_confirmation_observation_gap，使用DXGI发布源、WGC采集、CaptureAndOutput完成事件，启用现有采集timestamp诊断。这条诊断本来就有查询及其Flush开销，不作为无探针基线。40336最终Release测试编译完成；70732/61772两轮分别35.52s左右正常完成，capture observation及presentation records丢弃均0。原始报告保存target/annotation-confirmation-gap-report{1,2}.txt，原生执行日志annotation-confirmation-gap-native{1,2}.log。

| 指标 | 第1轮 | 第2轮 |
|---|---:|---:|
| 配对样本 |1799|1800|
| 查询可读观察→changed确认 P95(ms)|0.003000|0.003100|
| 间隔≥1ms样本 |0|0|
| 匹配OS显示版本FPS（下界）|59.668|59.834|
| OS显示版本QPC更新间隔P95(ms)|27.783|27.783|
| 仪表GPU执行P95(ms)|1.766|2.031|

两轮不支持已被CPU观察到完成的查询仍因确认排队多等一次刷新。它不覆盖GPU实际完成到首次CPU轮询前的空档，不排除共享GPU队列的上游等待，也不说明DWM内部合成取批；观察阶段P95不能相加/相减归因。生产策略未变，没有性能收益声明。下一GPU范围仍是主候选实际完成→首次观察、以及提交后合成边界；不据本轮直接拆设备/线程或删除正确性确认。

70950最终门禁exit0：annotation_gpu55通过/51忽略、workspace566通过/87忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint及Windows workspace Release（0.88s复用生产产物）、diff check通过。日志target/annotation-confirmation-gap-{related-all,workspace,clippy,release,diff}.log。新ignored原生诊断显式两轮执行单列；完整App/IME/负坐标/资源/物理显示及GPU第四次刷新归因仍未完成。AGENTS.md与DAG兼容，无Core接口/配置/依赖变化，无暂存、提交或推送。

## 2026-10-05：同一真实App马赛克绘制／完成与空会话对照

最新Release真实App owned PID32012，125%主屏工具条、100%副屏区域马赛克。8轮绘制均在副屏(3100,400)至约(3340,520)，输入按住移动后松开；每轮观察到GPU输出窗口，再点击完成。关闭后无Annotation Canvas/Preview/Options/GPU窗口，只有原有18×18后台窗口；进程持续Responding。原生图形输出HWND证据不等同逐像素效果验证、中文IME或物理显示延迟。第0轮先绘制较大区域，后7轮相同尺寸，首轮不能作严格像素/成本基线。

关闭后等待450ms记录target/annotation-app-resource-cycles.csv：handle 370,374,375,378,379,380,381,382；GDI均20、USER均30，visible均1，线程16至20；PrivateBytes约134.1至140.0MB，中间波动。这些快照不能宣称资源稳定或推算每轮精确泄漏。

后续空闲快照target/annotation-app-resource-idle.csv（首快照与最后绘制间另有工具/分析时间，不把标签0视为关闭后立即）：handle均381、GDI/USER20/30、线程16、PrivateBytes23384064；再等2秒、10秒不变。没有调用工作集修剪或人为Flush。

再做同进程8轮空会话，只打开并完成，不绘制；每轮打开后确认未创建GPU输出窗口。首轮固定250ms等待未观察到工具条，进程仍活且随后工具条出现；没有重启或再次发热键，用可见窗口的有界轮询续接该次打开。target/annotation-app-resource-empty-control.csv：handle每轮370，GDI/USER20/30，线程16，PrivateBytes23998464至24059904。后续操作/更长时间后先前额外handle已回收，不能将8次绘制的单调短期增长直接认定永久泄漏；也不能认定具体由空会话触发而不是异步退休时间。需要同等待预算与交替控制、句柄类型及释放时序证据，尚未满足完整资源验收。

最后标注窗口已关闭，清单target/annotation-app-resource-final-windows.txt，仅路径验证一致后Stop-Process清理本次启动的后台进程；不将强制停止当正常程序退出验收。没有源代码改动、没有重跑已通过70950源码门禁，文档diff check单列；无暂存、提交或推送。完整UI/IME/负坐标/资源与GPU额外刷新仍继续。

## 2026-10-05：WGC 0与1ms最小更新间隔反向对照，已撤回

关闭协议审计未发现明确遗漏：UI close使所有实例/焦点任务失效、清理Scene/Renderer Port；worker generation变化销毁Renderer/retained surfaces；WindowCapture先退订，再Close session/pool。源码顺序不证明系统内部已完成退休，不据此关闭资源问题。

参考Win32CaptureSample作者仓库用户报告 https://github.com/robmikh/Win32CaptureSample/issues/82 ：部分Windows版本上MinUpdateInterval低于1ms反而降帧。该报告不是微软普遍保证；本机现有生产值0已优于过去16ms默认，需要验证1ms是否还能改善。只在cfg(test)加入AtomicI64间隔开关与独立ignored入口，生产仍0；1ms分支逐会话读回属性并断言10000个100ns单位，日志确认设置实际生效。

本轮测试Release编译47989 exit0；实际原生26855四轮与88867两轮均terminal exit0，各35.5秒左右，顺序0→1ms→0→1ms→1ms→0。DXGI 60Hz源、WGC采集、同大模糊对象，CaptureAndOutput事件、采集timestamp关闭；生产算法/层序/帧池/呈现均一致。来源目录仍为当时桌面的真实窗口，环境与合成相位不固定；OS版本FPS为匹配下界，间隔非输入到物理显示。

| 轮次 | 间隔设置 | 匹配OS FPS | OS QPC间隔P95(ms) | 超过3次刷新比例 | 仪表GPU P95(ms) |
|---|---|---:|---:|---:|---:|
|0|0|54.534|34.728|0.117431|1.730|
|1|1ms|55.501|34.727|0.097957|1.827|
|2|0|54.433|34.728|0.111520|1.716|
|3|1ms|56.503|27.783|0.072609|1.768|
|4|1ms|56.402|27.783|0.070964|1.694|
|5|0|59.801|20.838|0.012828|1.691|

六轮capture observation/presentation records丢弃均0。前两组1ms看似改善，但反向组0显著更好，拒绝稳定收益声明，不能把最后0的一轮达标当整体性能验收。该开源问题在本机没有得到可重复的对应修复；以后不重复0/1ms猜测，不添加生产1ms限制。所有实验接线与独立入口已按字节快照撤回，capture.rs/performance.rs SHA256均与本轮基线一致；此前配对诊断保持。快照target/annotation-wgc-interval-experiment/{baseline,experiment}，native0..5日志及report0..5.txt保留。

25157最终源码门禁exit0：annotation_gpu55通过/51忽略、workspace566通过/87忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release和diff check通过，日志target/annotation-wgc-interval-final-{related,workspace,clippy,release,diff}.log。实验测试通过与最终源码门禁分别记录，不将临时入口留在发布源码。没有新增Core公开接口/配置/crate/依赖，无暂存、提交或推送。额外刷新、资源退休时序、完整App/IME/负坐标/物理显示仍未完成，AGENTS.md与DAG无冲突。

## 2026-10-05：主工具条／画布／参数面板统一原生指针重入路由

GPU Surface.present审计沿用已验证拓扑与已失败的位置跳过对照，没有重复修改SetWindowPos或增加呈现同步。交接审计发现主工具条pointer_main、画布pointer_canvas仍在原生回调内直接校验Registry；二级面板已有局部保护。将现有回归扩展为三个入口后，60496 baseline exit101在annotation.rs:4840 main_current复现RefCell already mutably borrowed；基线在主入口终止，不声称单独复现画布实机崩溃。

迁移为窗口InputWork::pointer_sink_when_ready：窗口组件负责可用条件、待回放数量、取消和桥接；协调器只提供Registry可借用的条件并路由。正常事件直接分发；重入事件进入同一窗口现有FIFO队列，有积压时后续原生事件也排队，防止松开越过被延后的按下。回放重新检查指针暂停/退休和可用条件，实例/会话校验仍由原接收器完成；weak队列记录不延长旧路由生命周期。主工具条、画布、参数面板共用，原面板局部递归保护删除。未更改平台钩子、Core行为、编辑实例或水印规则，没有新增业务状态副本。

相关回归覆盖三入口持有Registry可变借用时按下/松开、安全拒绝过期窗口、窗口退休取消；另验证正常事件立即收到、借用释放后但旧事件未回放时新事件不能插队、按原顺序回放。43009初版回归通过；36191初版门禁通过但随后发现顺序缺口，不当最终证据。最终20343 exit0：UI annotation226通过/9忽略、workspace566通过/87忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release及diff check通过。日志target/annotation-pointer-routing-{baseline,related}.log和-final-{ui,workspace,clippy,release,diff}.log。

最新Release真实App owned PID33604，125%主屏工具条／100%副屏画布，3轮切换序号点击创建、切换马赛克按住绘制；GPU输出窗口与工具条保持，进程Responding=True。面板处于主工具条上方（底边空间不足），截图target/annotation-pointer-routing-panel.png已查看：整行居中、无裁剪。窗口清单annotation-pointer-routing-app-windows.txt。点击完成后画布/GPU/参数/主工具条均销毁，closed-windows.txt只保留18×18后台窗口，进程仍活；仅按路径校验停止本次smoke进程清理，不当程序正常退出证据。没有验证中文IME、负坐标、逐像素效果或真实输入到物理显示。

本轮修复UI交接，不作为GPU第四次刷新改善。资源退休时序及系统合成归因仍待查，完整范围仍未完成；AGENTS.md与依赖DAG无冲突，无新增Core公开接口、配置、依赖或crate，无暂存、提交或推送。

## 2026-10-05：窗口指针重试保留队首，修复再次不可用时乱序

继续审计新InputWork共用路由，发现逐事件重试把旧事件重新追加至窗口工作队尾：按下回放时ready仍false、随后ready恢复，则后一松开可以先被消费。新增fake-clock回归pointer_retry_cannot_move_press_behind_release_when_readiness_changes，在9607 baseline exit101稳定复现release overtakes press；它验证接收器状态改变的受控协议，不声称完整App已实机复现这一时序。

PointerRoute改为自身的原生事件VecDeque和单个弱回放任务。暂不可用时保留队首，后续事件在其后；一轮只处理启动时队列长度，回调产生的新事件留到后续轮，避免无限同轮分发。正常无积压时仍直接分发。InputWork只保存路由Weak，retire清理队列、定时器和待处理任务；指针暂停时取消该轮队列，保持原暂停语义。没有新增业务状态或平台线程。前轮主/画布/参数面板回归继续保留。

19511最终门禁exit0：UI annotation227通过/9忽略、workspace567通过/87忽略、Clippy --workspace --all-targets -- -D warnings、UI build.rs/Slint、Windows workspace Release及diff check通过。日志target/annotation-pointer-head-{baseline,ui,workspace,clippy,release,diff}.log。

最新Release真实App owned PID34444，125%主屏工具条／100%副屏画布，3轮序号点击创建、切换马赛克按下/移动/松开绘制，进程Responding=True；面板截图target/annotation-pointer-head-panel.png已查看，单行居中、无裁剪。窗口清单app-windows.txt。点击完成后Canvas/GPU/Options/Preview均消失，closed-windows.txt只余18×18后台窗口；仅路径校验停止本次smoke进程，不当正常程序退出测试。没有验证实际ready再次变化的极端原生时序、IME、负坐标、逐像素效果或物理显示延迟。

本轮完善批次2窗口输入协议，不作GPU延迟/资源增长改善声明。GPU第四次刷新、资源退休时序与完整验收仍未完成，AGENTS.md/DAG无冲突，无新增Core公开接口/配置/crate/依赖，无暂存、提交或推送。

## 2026-10-05：按鼠标隔离思路审计等待，并减少首次模糊校准CPU工作

对照mouse-input-isolation.md：鼠标问题已定位安装线程的消息同步等待；本链路尚未定位同等级根因。27.783ms为相邻匹配OS显示版本更新间隔，不是某个OS API执行时长。已有通知回调与GPU工作隔离、额外来源Pending期间主来源可提交、查询可读观察到确认约0.003ms证据仍有效，但不覆盖实际GPU完成到首次轮询或DWM内部取批。生产completion_event名字实际是D3D11_QUERY_EVENT/GetData DONOTFLUSH轮询，并非ID3D11Fence通知。微软Immediate Context/DXGI线程约束要求序列化对应操作，不能把鼠标专用线程收益直接推广为拆GPU线程/设备： https://learn.microsoft.com/en-us/windows/win32/direct3d11/overviews-direct3d-11-render-multi-thread-intro 。下一GPU归因继续上述两个未测边界，不重复已撤回的主来源优先/1ms采集间隔/额外Flush实验。

发现参数路径中Plan::new首次数值校准同步占用worker。已有可见sigma缓存，不是每帧重算；本批保留缓存退休及GPU算法，只在校准重采样时跳过确定为零的输出区间。根据非零支撑、tap偏移及双线性邻点计算保守区间，端点非零保留相应clamp边缘；额外索引保护浮点边界。有效样本运算及求和顺序不变。原完整重采样保留cfg(test)，reference_signal改用它，避免GPU画质参考跟着优化自证。

新增纯回归覆盖长度1/2/3/16/32/33/1024、缩放方向、端点/中心脉冲、正负稠密信号及零信号；另有2–5级、四个校准相位、offset 0至8的每级逐位比较，均通过。元数据校准不扫描图像、不改变shader、sigma/混合/边缘padding/DPI/采集与呈现。未新增Core接口/依赖/crate/配置，AGENTS及DAG兼容。

Release编译99756原版、88479最终版均exit0。CPU测量单入口先预热3次再40次计时，无并行构建；原版一次与优化两次：

| sigma | 原版P50/P95 ms | 优化1 P50/P95 ms | 优化2 P50/P95 ms |
|---|---|---|---|
|5|2.288300/2.306000|0.354000/0.741500|0.351900/0.478600|
|11|4.977800/5.333100|0.927100/1.405600|0.924200/0.960700|
|22|5.578200/5.594000|1.424500/1.447500|1.430700/1.476800|
|25|2.839300/2.857500|0.815600/0.832900|0.822900/0.849900|
|60|2.949100/2.957400|1.447800/1.665600|1.470400/2.292700|

日志target/annotation-blur-calibration-{baseline,optimized-1,optimized-2}.log。这是Plan创建的CPU成本，不包含缓存命中、GPU执行、WGC、OS同步或真实输入显示；两轮均有收益而尾部波动仍保留。随后--include-ignored执行blur全部6项通过，包含实际D3D11强度/过渡/DPI脉冲、边缘及分块一致性；该次并行测试中benchmark输出不加入上表。日志target/annotation-blur-calibration-native.log。未重跑显示性能，不能宣称27.783ms或第四次刷新已改善，没有新增实际App截图。

最终76879 exit0：annotation_gpu57通过/52忽略、workspace569通过/88忽略、Clippy --workspace --all-targets -- -D warnings、Windows workspace Release（1m20s）及diff check通过。UI未修改，现有UI/Slint产物复用，无新独立Slint运行检查。日志target/annotation-blur-calibration-final-{build,related-all,workspace,clippy,release,diff}.log。GPU根因、资源退休时序、实际IME/负坐标及物理显示验收继续，整体目标未完成；未暂存、提交或推送。

## 2026-10-05：同候选查询未就绪到就绪的CPU轮询边界

沿鼠标钩子隔离的等待审计，检查d3d_compare的enqueue：CopyResource(readback,flag)后立即End(completion_event)，没有插入其他来源命令；它仍在共享Immediate Context队列中，不能排除此前排队工作或驱动内部依赖。GetData DONOTFLUSH本来就是轮询，不是ID3D11Fence通知。微软GetData契约S_FALSE表示调用方尚无可读数据，不等同精确硬件完成时刻：https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-getdata 。不能把旧GpuComparison timestamp查询的可读观察与completion_event实际轮询混为同一个点。

本批只在cfg(test)且已有主来源GPU诊断开启时，记录每次现有completion查询调用的began/finished/ready；没有额外GPU查询、Flush、线程、阻塞等待或生产计时。Source带冻结Published(token,sequence,ticks)转发，沿用128条/来源和131072条会话的有界事件队列。离线只匹配同主来源、唯一changed确认及其生命周期内的有序调用；需要not-ready前驱及首次ready，拒绝跨候选/来源、重复确认、重叠或乱序、倒置时间和晚于确认。纯回归覆盖这些条件。记录是CPU数据可用性括号；不证明GPU绝对完成、队列等待或物理显示。查询ready但Map仍Pending时也只取首次ready。

Release编译96571/59814均exit0，最终59814为确认过滤后的源码。14795两轮各约35.5秒原生诊断正常完成，DXGI60Hz源、WGC、大模糊、CaptureAndOutput、既有capture timestamp开启；不并行构建。

|指标|轮1|轮2|
|---|---:|---:|
|同主候选timestamp观察→changed确认样本|1800|1800|
|上述确认间隔P95 ms|0.003200|0.003000|
|有效not-ready→首次ready括号数|91|140|
|最后not-ready调用开始→ready调用结束P95 ms|3.391000|3.688800|
|上述括号最大 ms|3.951600|5.141200|
|最后not-ready调用结束→ready调用开始P95 ms|3.390600|3.688000|
|上述调用间空档最大 ms|3.951000|5.140800|
|调用间空档≥1ms样本|81|109|
|capture/presentation记录丢弃|0/0|0/0|
|匹配OS显示版本FPS下界|59.965|59.734|
|OS显示版本QPC间隔P95 ms|27.783|27.783|
|仪表GPU执行P95 ms|2.106|2.208|

原始报告target/annotation-query-poll-report{1,2}.txt，执行日志annotation-query-poll-native{1,2}.log。这些括号只是有未就绪前驱的子集，不假设其余候选全是首次ready，也不推广到全部版本。未测无探针对照，不把诊断FPS作为优化收益；probe沿用timestamp及原诊断Flush，开销边界保留。最大值未达到一个实测144Hz周期，不能据此认定第四次刷新已被解释或排除。值得追查的是实际调用空档比SOURCE_RETRY_DELAY=0.5ms长；下一步将同候选括号和已有worker阶段/WaitSample配对，分开处理剩余工作、计时器请求与实际等待、唤醒后调度。不要将独立P95相减推定具体开销，勿直接缩短延迟或忙等。

Release GPU精确比较回归（8/16线程组、completion关闭/开启、RGBA/ROI及基线复用）显式通过，target/annotation-query-poll-exact.log；相关annotation_gpu58通过/53忽略。UI未改，无实际App截图或新IME/负坐标证据。本批没有Core接口/配置/依赖/crate变更，AGENTS及DAG兼容；第四次刷新、资源退休时序及完整实机/物理显示验收仍未完成，未暂存、提交或推送。

最终99552源码门禁exit0：workspace570通过/89忽略、Clippy --workspace --all-targets -- -D warnings、Windows workspace Release（1m10s）和diff check通过；现有UI/Slint产物复用，未执行新独立Slint检查。日志target/annotation-query-poll-final-{build,related-all,workspace,clippy,release,diff}.log。整体目标保持未完成。

## 2026-10-05：同候选轮询间隔中的worker与原生等待配对

上一批仅知道0.5ms请求与3.3–3.7ms实际查询间隔不一致，本批将query_poll_intervals保留最后not-ready调用结束与首次ready调用开始的Instant边界；query_poll_brackets继续生成原指标，不改变配对身份规则。新增query_gap_work_report，以实际交集裁剪Attempt.at/work及WaitSample.before/after，排序后拒绝同类和跨类重叠，剩余记unassigned；每个样本work+wait+unassigned等于该样本间隔，不相减独立分位数。完整且Timer唤醒的wait另计requested与max(actual-requested,0)，部分边界等待不推定完整请求/超时，Signal唤醒不当Timer。raw rows保留五列ms（work,wait,unassigned,完整Timer请求合计,完整Timer超时合计），允许逐样本复核。

新增WaitsOnly诊断模式，只开启已有Clock WaitSample记录，不额外开启DComp统计查询；新的ignored入口gpu_live_capture_query_gap_worker_wait_partition保持旧轮询入口不变。纯回归验证两端裁剪、多类相邻、不重叠完整分解、重复/跨类重叠拒绝及部分Timer不计完整超时。所有改动在既有cfg(test)性能模块中，无生产计时、线程、查询、调度或缓存策略变更。AGENTS及七crate依赖DAG兼容，无Core公开接口/配置/依赖变化。

35492 Release编译exit0。32728两轮各约35.5秒原生诊断正常完成：DXGI60Hz源、WGC、大模糊、CaptureAndOutput、主源timestamp及现有paint probes开启，WaitsOnly不启用composition probe；无并行构建。报告target/annotation-query-partition-report{1,2}.txt，日志annotation-query-partition-native{1,2}.log。

|指标|轮1|轮2|
|---|---:|---:|
|同候选not-ready→ready有效间隔|70|131|
|分解valid/invalid|70/0|131/0|
|查询调用间隔P95 ms|3.319000|3.300800|
|同间隔worker work P95 ms|1.094100|0.959600|
|同间隔native wait P95 ms|2.343100|2.434500|
|同间隔unassigned P95 ms|0.027800|0.022700|
|完整Timer requested合计P95 ms|0.992300|1.343900|
|完整Timer actual超过requested合计P95 ms|1.301866|1.263500|
|逐样本wait比work/unassigned都大的数量|60|126|
|逐样本Timer超时合计≥0.5ms数量|28|71|
|capture/presentation/nativeWait记录丢弃|0/0/0|0/0/0|
|匹配OS版本FPS下界|59.935|58.934|
|OS QPC版本间隔P95 ms|27.783|27.783|

P95列不能相加：不同样本的分位数对应不同间隔。等待包括进入等待至返回的墙钟时间，超时合计也包含OS唤醒/调度等，不是单个API内部执行时长、精确硬件完成时刻或物理显示。该子集不能覆盖没有not-ready前驱的候选；仪表有既有timestamp/Flush/paint开销，没有无探针对照，不作生产收益声明。未覆盖很小支持本受控子集中没有额外几毫秒的未记录CPU尾部，但不能排除App竞争或GPU队列依赖。仍须追查每个间隔内等待次数/来源、capture尾部与下一capture前工作；请求合计可以跨多次等待，不能把0.992/1.344ms都解释为单次SOURCE_RETRY_DELAY。不要直接改变全局timer精度、忙等或重做已失败的Fence/prefetch策略。GPU第四次刷新、完整App/IME/负坐标/资源及物理显示仍待验收，未新增截图、未暂存/提交/推送。

最终28237源码门禁exit0：annotation_gpu59通过/54忽略、workspace571通过/90忽略、Clippy --workspace --all-targets -- -D warnings、Windows workspace Release（0.66s复用生产产物）及diff check通过；UI/Slint产物复用，本批未改UI，无新独立Slint运行检查。日志target/annotation-query-partition-final-{related-all,workspace,clippy,release,diff}.log；原生35492测试编译/32728两轮测量单列。整体目标未完成。

## 2026-10-05：查询间隔的等待来源与CPU阶段归属

上一批工作/等待总量已配对，本批复用performance_interval的preceding_outcome及OUTCOME_NAMES，不复制等待分类。新增Stages.cpu_spans五个CPU边界，由已有stage start及elapsed形成，无额外计时API调用；字段/赋值均cfg(test)。query_gap_origin_report先通过原partition_gap，再裁剪阶段及等待实际交集；阶段须处于Attempt本体内、阶段互不重叠，stage合计不得超过work，其余记work_outside_stages。等待分类取wait.before之前最近且唯一完成的Attempt；分类不是当前GPU状态或OS唤醒原因。保留逐行14列（8种等待后果、capture/effects/compose/compare/commit、stage外work），便于同样本复核而非分位数相减。

新增纯回归覆盖output_retry/source_comparison分类、部分capture边界及阶段外工作、阶段重叠拒绝。所有变化仅测试观测/离线报告，生产调度、采集、效果、呈现均未改变；未新增Core公开接口、配置、依赖或crate，AGENTS/DAG兼容。

14098 Release测试编译exit0。32253两轮各约35.5秒正常执行现有gpu_live_capture_query_gap_worker_wait_partition入口，不新增重复ignored入口。DXGI60Hz源、WGC、大模糊、CaptureAndOutput、paint/capture timestamp、WaitsOnly；不新增composition统计探针，无并行构建。日志target/annotation-query-origin-native{1,2}.log，完整报告annotation-query-origin-report{1,2}.txt。

|指标|轮1|轮2|
|---|---:|---:|
|origin有效间隔/invalid|97/0|78/0|
|调用间隔P95 ms|2.191900|1.898000|
|来源比较后wait calls|151|118|
|输出重试后wait calls|5|2|
|有来源比较后wait的间隔|94|74|
|有输出重试后wait的间隔|3|1|
|其他六类wait calls|0|0|
|来源比较后wait交集P95 ms|1.206200|1.204600|
|capture CPU交集P95 ms|0.784100|0.913700|
|stage外work交集P95 ms|0.044200|0.052400|
|effects/compose/compare/commit交集P95 ms|0/0/0/0|0/0/0/0|
|上述后续阶段任一非零的间隔|4|1|
|capture/presentation/Wait记录丢弃|0/0/0|0/0/0|
|匹配OS版本FPS下界|59.000|59.903|
|OS显示版本QPC间隔P95 ms|27.783|27.783|

调用次数可跨多个wait，不是候选数；8类分类互斥但每个间隔可有多类。P95不能相加；P95=0不等于所有样本为零，已单列少数后续阶段交集。诊断间隔比前批短，生产行为没有变化，环境/相位与诊断扰动尚未控制，不作优化收益。仅覆盖有not-ready前驱的子集；query可读仍不是绝对硬件完成时刻或物理显示。本受控子集多数等待确实跟在来源比较之后，不支持把输出重试当普遍主因，不能排除少数输出路径或真实App竞争。

源码审计：Backend.sample_gpu在来源确认重试仍调用windows(request)并整理crops/visible/signature；Renderer.render对CaptureLayer仍Texture::wrap(...sampled=true...)，创建SRV。当前capture阶段0.78–0.91ms不等于这两个子操作的独立耗时，下一步需实际子阶段边界，复用现有枚举探针并覆盖窗口移动/遮挡/创建/关闭、尺寸变化及设备恢复后再做有界复用实验。不能直接冻结窗口目录，过去目录freeze/输出prefetch实验未提供稳定OS收益，勿重复相同假设。Timer超时、GPU队列、DWM取批及第四次刷新仍未定位；完整App/IME/负坐标/资源/物理显示未验收，未新增截图，未暂存/提交/推送。

最终82150源码门禁exit0：annotation_gpu60通过/54忽略、workspace572通过/90忽略、Clippy --workspace --all-targets -- -D warnings、Windows workspace Release（1m11s）及diff check通过；UI/Slint产物复用，未新增独立Slint运行检查。日志target/annotation-query-origin-final-{related-all,workspace,clippy,release,diff}.log；14098测试编译/32253两轮原生诊断单列。整体目标继续，未标记完成。

## 2026-10-05：采集窗口枚举/纹理Wrap细分及查询顺序负实验

上一批capture CPU交集约0.78–0.91ms，本批在Backend原枚举探针上保留start+原elapsed边界；Renderer复制到Stages.capture_details[0]。Texture::wrap创建SRV的每层CPU边界放在paint-probe.enqueue之后，不把诊断paint读回算作视图创建。固定记录16个Wrap区间及一个枚举区间，超容量标记invalid，不分配无限记录。仅现有enumeration_probe开启时读时钟；注入帧同时清空旧枚举边界，Backend每轮也清空。新WaitsAndEnumeration复用WaitsOnly/原枚举探针，不启用composition统计。报告仅在对应探针启用时生成，不能把未启用探针的0误当实际耗时。

capture_details要求所有子区间在所属完整capture阶段中、不重叠，精确裁剪查询调用间隔交集；其余为capture合计减该同样本枚举/Wrap合计，不相减独立P95。早退/缺少父阶段或溢出/异常区间保守排除，单独invalid；没有猜测其耗时。纯回归覆盖两端裁剪、阶段内/外、重叠及溢出拒绝。生产仍执行相同Texture::wrap调用、无生产计时/查询/缓存/线程策略变化，普通字段读取移到局部纹理创建之后不改采集对象参数；无Core公开接口/配置/依赖/crate变化，AGENTS/DAG兼容。

76200初次Release测试编译、24799探针限定后的最终Release测试编译均exit0。21567两轮各约35.5秒原生测量正常完成，DXGI60Hz/WGC、大模糊、CaptureAndOutput、原paint/capture timestamp、WaitsAndEnumeration；无并行构建。日志target/annotation-capture-details-native{1,2}.log，报告annotation-capture-details-report{1,2}.txt。

|指标|轮1|轮2|
|---|---:|---:|
|细分valid/invalid|94/1|228/0|
|调用间隔P95 ms|2.059000|2.704800|
|原capture阶段交集P95 ms|0.773500|0.730300|
|枚举交集P95 ms|0.750200|0.697000|
|Texture::wrap交集P95 ms|0.017100|0.015700|
|同capture其余交集P95 ms|0.026800|0.026900|
|逐样本枚举大于另外两项的数量|94|228|
|capture/presentation/Wait记录丢弃|0/0/0|0/0/0|
|匹配OS版本FPS下界|58.671|58.301|
|OS版本QPC间隔P95 ms|27.783|27.783|

第一轮一个invalid的具体排除原因没有细分记录，不能假称全部路径都有父阶段或已覆盖其耗时。P95不能相加，capture列与细分列样本数量也不同。枚举整体包括EnumWindows、可见性、DWM属性、区域交集、opacity和目录整理，不等于单个DWM API耗时；当前结果支持优先枚举路径而非把0.016ms Wrap预设为毫秒主因。仪表增加CPU边界/较大的Stages记录，沿用原timestamp/Flush；没有无探针对照，数据不是产品输入/物理显示或零开销证明。

### bounds-before-cloak单因素实验与撤回

参考DwmGetWindowAttribute官方属性读取契约 https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/nf-dwmapi-dwmgetwindowattribute ，不假定缓存名称/边界永久有效。只在cfg(test)开关改变枚举次序：仍完整检查可见、cloaked、DWM extended bounds、区域交集、opacity和z-order；实验先读bounds排除区域外窗口再读cloak。基线保持cloak先行；开关每次枚举读取一次，测试RAII恢复，生产仍旧次序。没有跳过相关窗口cloaked检查或使用不可靠GetWindowRect替代DWM边界。

70779 Release实验编译exit0。25785先experiment1、baseline、experiment2各原生执行：baseline短名称筛选同时匹配两个ignored测试（日志running 2 tests），存在并行竞争与共享开关污染，整轮数据弃用，不作为任何性能结论。保留annotation-enumeration-order-baseline.{log,txt}，不把它的20.839ms结果当优化证据。32603用完整windows::annotation_gpu::performance::gpu_live_capture_query_gap_capture_details加--exact重跑，日志确认running 1 test，35.76s exit0。

|有效顺序数据|枚举交集P95 ms|细分valid/invalid|OS版本间隔P95 ms|匹配OS FPS|
|---|---:|---:|---:|---:|
|原探针基线1|0.750200|94/1|27.783|58.671|
|原探针基线2|0.697000|228/0|27.783|58.301|
|experiment1|0.775200|86/0|27.783|59.901|
|experiment2|0.712200|52/0|27.783|59.935|
|相同实验实现的exact基线重跑|0.845700|96/0|27.783|58.900|

旧与exact基线存在明显波动，不能只挑最后一轮声称CPU改善。实验没有在多个基线中呈现稳定关系，也没有刷新尾部收益，因此完整撤回两文件改动：capture.rs/performance.rs SHA256分别与实验前快照一致。快照target/annotation-enumeration-order-experiment/{baseline,experiment}；日志annotation-enumeration-order-{build,experiment1,baseline,experiment2,baseline-exact}.log，对应txt保留。最终发布源码无BOUNDS_BEFORE_CLOAK_EXPERIMENT、无bounds_first测试入口/分支，原排序恢复；细分探针与纯回归保留。

下一范围从windows(request)内查询组成及失效条件入手，确认同请求来源重试能避免哪些重复准备。不得盲目冻结目录、移除遮挡/移动检测、增加常驻隐藏窗口/设备或只因Wrap可缓存就预设显示收益；既有目录freeze/输出prefetch失败证据保留。GPU第四次刷新、完整App/IME/负坐标/资源及物理显示验收仍未完成，无新截图、未暂存/提交/推送。

最终20620源码门禁exit0：annotation_gpu61通过/55忽略、workspace573通过/91忽略、Clippy --workspace --all-targets -- -D warnings、Windows workspace Release（1m16s）及diff check通过；UI/Slint产物复用，未执行新独立Slint检查。日志target/annotation-capture-details-final-{build,related-all,workspace,clippy,release,diff}.log。两个实验文件恢复哈希及零临时入口再次核验；原生/弃用轮次分别记录，整体目标未完成。

## 2026-10-05 优先修复：绘制后鼠标穿透失效

用户要求先修复绘制后的鼠标穿透。实际App基线复现：创建标注后切换鼠标模式，Canvas扩展样式包含LAYERED+TRANSPARENT，但WindowFromPoint(600,450)仍命中独立GPU窗口（App PID30968，GPU HWND2229748，style82000A8）。GPU窗口原来只有WS_EX_TRANSPARENT和WM_NCHITTEST返回HTTRANSPARENT，后者只查同线程窗口，不能保证其他应用穿透。给受控App的GPU窗口临时加入LAYERED并以alpha255初始化后，命中立即返回底层其他应用；仅操作本次启动的App，完成后销毁会话并停止自有进程。

修复在annotation_gpu/surface.rs：GPU视觉始终使用WS_EX_LAYERED|WS_EX_TRANSPARENT，CreateWindowEx成功后SetLayeredWindowAttributes(alpha255,LWA_ALPHA)，失败沿用DestroyWindow清理。保留DirectComposition、交换链、窗口层序及独立Slint输入画布，没有隐藏窗口或软件绘制回退。Windows依据：https://learn.microsoft.com/en-us/windows/win32/inputdev/wm-nchittest 、https://learn.microsoft.com/en-us/windows/win32/winmsg/window-features 、https://learn.microsoft.com/en-us/windows/win32/directcomp/basic-concepts 。

performance.rs现有ignored原生像素测试增加另一线程的可交互底层夹具，Drop通知并join关闭；连续四种颜色版本仍精确显示，每次在两个不透明像素位置断言WindowFromPoint到达底层。调试夹具时发现STATIC无SS_NOTIFY自行穿透，以及未设每屏DPI的线程在125%屏幕上原45,45测试点落在夹具外；设置SS_NOTIFY并将两点限定为实际夹具内104,104和80,80，前置命中断言确认底层可交互。最终9083原生Release测试1通过（0.27s），target/click-through-native.log。该测试验证跨线程命中，不声称它本身执行了真实跨进程鼠标输入。

真实Release App受控测试2c7c54 exit0：独立PowerShell进程的Windows Forms测试窗口放在App底下；125%屏幕绘制矩形后，在线条600,450及空白900,600点击计数1、2；返回编辑后App接收线条点击且底层计数不变；再切穿透线条点击计数3。移动底层窗口至100%第二屏，创建矩形后在线条3000,450及空白3300,600点击计数4、5。每次输入前检查WindowFromPoint属于预期受控进程，并检查底层MouseDown计数恰增1。结束按钮之后Canvas/GPU窗口均消失；最后只停止本次App进程并关闭夹具。截图target/click-through-fixed-{125,100}.png已查看，标注线条保留、无裁剪。当前实机没有负坐标屏幕；未重复所有工具、IME或性能延迟测试，不将本次结果扩展为完整验收。

门禁：9083 Windows workspace Release1m14s exit0；3300 workspace573通过/91忽略及Clippy --workspace --all-targets -- -D warnings exit0；diff check exit0（仅已有CRLF提示）。日志target/click-through-{native,release,workspace,clippy}.log。UI/Slint源码未改，编译产物复用，未另跑独立Slint检查。未暂存、提交或推送；此前GPU等待及资源问题继续保留在重构范围。

## 2026-10-05 优先修复：马赛克编辑碰到工具条停更

用户确认工具条覆盖期间继续实时更新，替代之前冻结覆盖区域的要求。原回归22388在修改前失败（target/mosaic-toolbar-before.log）：软件请求带有工具窗口暂停区域。代码因采样范围相交跳过整块效果，移动/缩放失效的缓存又阻止整帧呈现；模糊的外围采样边距使“靠近”也能触发。此前完整输出保护本身正确，本次不降低就绪判定。

实现：annotation.rs编译原生Scene固定paused_regions为空，删除native_paused_regions；annotation_mosaic_session移除标注会话的暂停区域状态/策略，请求同样为空。capture_exclusions统一收集画布、主/二级工具条、编辑、水印和当前菜单（含选色器）窗口，并排序去重。Presenter改为跟踪excluded_windows；原准备/定位通知改为比较排除集合，补齐菜单显示/关闭和面板/水印退休的集合变化通知。窗口移动不因几何覆盖暂停采样。Core/Platform通用paused_regions语义、GPU标记排除、最后成功画面、主动暂停与完整输出确认保持；无Core接口、配置或依赖变动。

回归：软件排除测试断言暂停为空，移动二级面板到负坐标相交范围不重复请求，替换Canvas重新发起正确排除集合。Native交接测试断言同时存在工具条/面板时Scene暂停为空且排除集合正确；新增Presenter排除替换/退休测试，确保相同对象/显示内容仍重新提交集合变更。原取消、捕获丢失、连续参数撤销等UI测试仍通过。首次28603 UI270通过/9忽略；最终73140 workspace574通过/92忽略、Clippy --workspace --all-targets -- -D warnings和Windows workspace Release（1m06s）exit0，target/mosaic-toolbar-final-{workspace,clippy,release,diff}.log。

新增ignored原生excluded_toolbar_keeps_covered_source_pixels_live：同位置完全不透明紫色工具窗口覆盖80×60底层源，排除工具窗口且不暂停，三种底层颜色均在内部60×40像素区域精确更新。初次夹具失败是STATIC初始重绘/边缘像素约束导致；补齐持续GDI发布和内部区域验证，保留中心观测诊断。最终debug原生测试exit0，0.36s，target/mosaic-toolbar-native-capture.log；未修改生产采集逻辑。既有GPU主动暂停/恢复/层序原生测试（0.76s）和四版本真实像素/跨线程穿透测试均通过，target/mosaic-toolbar-native-{effects,input}.log。b5bf62直接运行当前UI build.rs二进制，OUT_DIR隔离至target/mosaic-toolbar-slint-check，重新编译ui/main.slint及全部翻译，exit0；target/mosaic-toolbar-slint-check.log。没有改动Slint源码。

实际Release App使用独立进程的自有Windows Forms棋盘源：主屏125%工具条约650,650，副屏100%工具条约3057,607。区域马赛克强度100，先创建再整体移动到主/二级工具条下面，拉伸跨过工具条、移出，再选Blur强度100；原画布捕获持续收到跨工具条移动及松开，主工具未被该拖动切换。像素采样避开工具条和操作点：125%马赛克蓝相位[27,53,167]、Blur绿相位[26,165,52]；100%对应[26,53,167]/[26,165,52]，证明覆盖期间底层变化继续输出。

笔刷另在原区域之外绘制竖线跨工具条并移动；初始脚本曾在旧区域内部开始而移动了旧对象，又误取控制点，相关失败不算产品回归证据。校正绘制点、避开控制点、移动鼠标离开采样处后，89449主屏绿/蓝相位[56,254,112]→[56,112,254]，22026副屏[26,165,52]→[26,52,165]；两轮最终exit0，工具条仍展开，结束按钮后Canvas/GPU窗口均销毁，再停止路径核验后的自有App进程并关闭夹具。核心捕获取消和每次操作撤销回归使用workspace测试；未额外实机验证笔刷缩放和真实捕获丢失。

截图target/mosaic-toolbar-{before-overlap,overlap-green,overlap-blue,after-leaving,resized-overlap,blur-overlap,brush-crossing,brush-moved,brush-live}{,-100}.png，已查看关键区域/笔刷及双屏图像；不截取非自有应用内容。负坐标仅纯/UI测试覆盖，本机无负坐标屏幕；未测实体显示延迟、真实IME或完整资源稳定性，不替代原有未验收项。用户范围只修本次停更；不暂存、提交或推送。
# 标注菜单层级修复（2026-10-05）

新增 Platform 内部 annotation_window_order，按 HWND 属性实例登记 Canvas、GPU Visual 和具有 owner 链的 Tool；窗口销毁移除登记，消息同时验证 HWND 属性与实例。通过窗口定位前约束和事件触发的协调维持 Tool > Canvas > Visual，并按 owner 深度排列工具窗口。跨线程 Visual 使用带实例的合并消息，在自身线程调整；不从 UI 同步等待渲染线程，不在鼠标移动或普通像素呈现路径枚举/反复提升窗口。新旧保留的 GPU HWND 均参加登记。GPU present 只在首次成功提交后显示/定位，不再每帧 SetWindowPos；NOACTIVATE/NOOWNERZORDER 保持输入、捕获和窗口 owner 的稳定性。

UI 主工具条、参数面板、文字/水印编辑及三级菜单检查所属关系，显示后再次确认；失败采用现有 16ms、最多 20 次的有界准备重试。工具条 owner 缓存包含 Canvas 和 Toolbar 实例 revision，窗口替换后重新绑定。旧会话/组件的重试不操作重开的窗口。修复过程中补齐原生关闭监控实例及事件起始时间验证；沿用现有钩子线程，保留 WinEvent 原始发生时间，拒绝新监控之前的事件和旧实例的关闭消息。桥接补齐 WM_LBUTTONDBLCLK，确保快速点击的第二个 down/up 成对到达 Slint。

实机截图复核还确认了原生显示过渡：菜单在显示初期淡入，绘制元素暂时透过菜单，稳定后恢复不透明。仅对登记的标注 Tool HWND 设置 DWMWA_TRANSITIONS_FORCEDISABLED；不修改系统动画设置、Slint 配色、布局或其他应用窗口。最终菜单不再经过该淡入阶段。

回归先在修改前失败：rapid_menu_reopen_stays_above_other_monitor_canvas 第 0 次菜单命中被另一个 Canvas 遮挡。修复后连续 100 次新菜单/GPU 替换通过，检查 native tier 顺序、WindowFromPoint、实际白色菜单像素、旧实例消息、销毁后的登记数量和前台窗口不变。既有 GPU 四版本真实像素/跨线程穿透、马赛克层序与暂停保留测试继续通过。新增 UI owner 失败/重试/旧实例/耗尽清理测试，更新编辑窗口与菜单的显示后 owner 确认断言；新增旧关闭事件与 DWORD 时钟回绕回归。最终 workspace 577 通过/93 忽略、Clippy all-targets -D warnings、Windows workspace Release（1m13s）通过。日志 target/annotation-order-{workspace,clippy,release,native,input,effects}.log；直接在 lexift-ui 工作目录执行 build.rs，隔离 OUT_DIR 重新编译 Slint 与翻译，通过，target/annotation-order-slint-check.log。

实际 Release App 使用自有棋盘窗口，覆盖实时马赛克、模糊、主屏 125% 和副屏 100%。早期脚本以固定延时推断开关状态且未等待显示过渡，相关失败不能当作稳定层级失败；最终脚本按观察到的可见窗口状态同步，少量输入尝试需要重试，因此不作为极短间隔点击零漏点的证明。最终每屏 25 次成功显示均验证 native 顺序、鼠标命中及不透明菜单像素；此前还完成每屏 50 次显示验证。跨屏工具条跨越 x=2560，菜单按工作区夹到主屏边缘；字体菜单和选色器（125%）在副屏 owner 的工具条（100%）下打开，均位于两屏 Canvas/GPU 之上且可命中。字体/颜色菜单 Escape 后关闭，结束会话后 Canvas/GPU 均销毁，自有 App 和夹具随后停止。截图 target/annotation-order-{menu-125,menu-100,cross-screen,font-cross-screen,color-cross-screen}.png，已查看；字体/颜色截图裁取自有菜单内部，避免截取外部应用。跨屏脚本必须使用各窗口自身 DPI，不能将工具条 100% 坐标比例用于 125% 参数面板。

本机无负坐标屏幕，仅使用已有负坐标 UI/定位回归；未新增真实 IME 或显示延迟验收。无 Core 公开接口、配置、依赖或 crate 变化；未暂存、提交或推送。原 GPU 延迟/资源等重构未验收项保持未完成。

