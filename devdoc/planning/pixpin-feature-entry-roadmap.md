# PixPin 功能入口对照与 Lexift 实施顺序

本表以 PixPin 官方使用手册核对用户能看到的入口和行为。涉及内部实现的结论来自仓库已有 PixPin 3.5.5.1 实机记录和 REA／IDA 证据。在线手册中的新增行为尚未逐项在本机复测。Lexift 状态以 `main` 的 `016de88` 为基线。

## 入口规律

- 托盘和全局快捷键负责启动独立任务，例如截图、从剪贴板贴图、找回已关闭的贴图。PixPin 文档默认分别为 Ctrl+1、Ctrl+2、Ctrl+3。Lexift 沿用自己的快捷键配置和冲突规则。 [快速上手](https://pixpin.cn/docs/getting-started/quick-start)
- 截图框选后，当前选区的标注、OCR、长截图、录屏和输出集中在截图工具条。较少用的动作可放在副工具条或更多菜单。 [基础截图](https://pixpin.cn/docs/screenshot/basics) · [截图设置](https://pixpin.cn/docs/settings/screenshot)
- 长截图和录屏各有自己的控制条。处理单张贴图的动作放在该贴图右键菜单，跨贴图动作放在托盘。设置负责默认值、快捷键和入口配置。 [长截图](https://pixpin.cn/docs/screenshot/long-screenshot) · [屏幕录制](https://pixpin.cn/docs/screenshot/screen-recording) · [贴图基础](https://pixpin.cn/docs/pin/basics) · [快捷键与动作](https://pixpin.cn/docs/settings/shortcuts-actions)

## 功能和入口

| 功能 | PixPin 的入口与行为 | Lexift 现状与缺口 |
| --- | --- | --- |
| 截图与标注 | 全局截图键、托盘；选区工具条提供标注和输出。[快速上手](https://pixpin.cn/docs/getting-started/quick-start) · [标注](https://pixpin.cn/docs/annotation/basics) | 已有全局截图、窗口候选、区域复用、标注、刷新和输出。 |
| 异形与多窗口截图 | 截图中切换矩形、自由、多边形和多窗口等模式；支持选区加减。[截图模式](https://pixpin.cn/docs/screenshot/modes) | 现有主流程为矩形选区；异形遮罩与多区域输出需单独设计。 |
| 自定义与延时截图 | Alt+1 打开自定义截图，可预设位置、尺寸和延迟。[自定义截图](https://pixpin.cn/docs/screenshot/custom) | 已有延迟截图和 R 复用；可命名、可选择的预设可独立补齐。 |
| 长截图采集 | 框选后工具条或 L；控制条选择纵向／横向，A 切换自动滚动。[长截图](https://pixpin.cn/docs/screenshot/long-screenshot) | 已有纵向手动、自动滚动和固定页眉页脚处理；横向支持单屏手动向右采集。 |
| 长截图采集内裁剪 | 采集控制条的裁剪菜单，J／K 以当前采集帧为基准裁掉前方或后方已拼接内容；完成后另有编辑入口。[长截图](https://pixpin.cn/docs/screenshot/long-screenshot) | 已支持纵向和横向采集内裁剪，以及成品编辑器裁剪。 |
| 长图结果编辑 | 完成后可从长截图控制条进入编辑，也可直接输出。[长截图](https://pixpin.cn/docs/screenshot/long-screenshot) | 已有独立编辑器、平移、缩放、裁剪、方向变换和尺寸调整。 |
| 快速保存与自动保存 | 工具条快速保存按配置直接写入；设置可配置路径和命名，另有按事件触发的自动保存。[保存设置](https://pixpin.cn/docs/settings/save) | 已有 PNG／JPEG 保存对话框；可先增加用户主动触发的快速保存。 |
| 普通贴图 | 框选后贴图，或从剪贴板贴图；右键管理当前贴图。[贴图基础](https://pixpin.cn/docs/pin/basics) | 已有图片贴图、移动、缩放、标注、裁剪、方向和颜色处理、OCR、锁定、穿透、透明度及批量显隐。 |
| 恢复已关闭贴图与分组 | 快捷键或托盘找回已关闭贴图；分组从托盘管理。[贴图历史](https://pixpin.cn/docs/pin/history) · [贴图分组](https://pixpin.cn/docs/pin/groups) | 已有图片输出历史和存活贴图显隐；可先补关闭后的贴图恢复，再考虑分组。 |
| 窗口贴图 | 截图副工具条或 Alt+W；持续跟随来源窗口的局部画面，最小化时暂停，可冻结为普通贴图。[窗口贴图](https://pixpin.cn/docs/pin/window) | Windows 已接入截图编辑「更多 → 窗口贴图」，支持单张实时窗口贴图、源最小化恢复与冻结为普通贴图。双屏正式验收见[接入记录](../releases/window-pin.md)。 |
| 多种贴图内容 | 官方列出图片、文字、颜色、文件、GIF 和 LaTeX 等类型；文件贴图有独立入口。[认识 PixPin](https://pixpin.cn/docs/getting-started/about-pixpin) · [文件贴图](https://pixpin.cn/docs/pin/file) | 已有 PNG／JPEG 打开、剪贴板图片贴图和托盘「从剪贴板贴文字」。文字贴图支持选择复制、置顶和统一显隐。 |
| OCR 结果 | 截图或贴图可从识别入口进入结果窗口，支持查看和选择复制。[文字识别](https://pixpin.cn/docs/recognition/text-recognition) · [快速上手](https://pixpin.cn/docs/getting-started/quick-start) | 已有截图／贴图识别复制及独立 OCR 翻译；可在识别复制之外补结果查看与局部复制。 |
| 录屏及录后处理 | 框选后从工具条或 G 启动；录制控制条及成品预览分工。手册列出首尾裁剪、画面裁剪、变速、音量及 MP4／GIF／WebP。[屏幕录制](https://pixpin.cn/docs/screenshot/screen-recording) | 已有音频、倒计时、30／60fps、快捷键、预览、首尾与画面裁剪、1×／1.5×／2× 变速、MP4 音量与移除音轨，以及 MP4／GIF。 |

## 已核实的内部实现

| 路径 | 已有本机证据 | 实施参考 |
| --- | --- | --- |
| 截图启动与元素识别 | 已追到 WGC／D3D11 采集路径、短期资源复用，以及鼠标位置变化后按需查询元素的路径。[启动分析](pixpin-startup-ida-analysis.md) | Lexift 已采用后台按点查询，维持首帧和输入独立于元素查询完成。 |
| 长截图 | Qt 控制层、定时驱动和滚轮模拟分工；采集图经匹配处理后按块追加，独立预览标记当前采集帧。长截图所用 `grabScreen` 下层采集 API 尚未确认。[长截图分析](pixpin-long-screenshot-research.md) | 复用 Lexift 已有采集、配准和分块资源，在确认结果模型内增加裁剪。PixPin 手册建议选区避开固定栏，Lexift 的固定栏处理是已有实现。 |
| 长图编辑 | 已核对图片位置、缩放和拖动交互，相关视口路径见专项记录。[编辑器分析](pixpin-image-editor-research.md) | 保留现有统一视口和原图坐标。 |
| 屏幕录制 | 已追到 WGC 自由线程帧池、D3D11 映射和行复制，以及队列背压和 x264 路径。硬件能力与实际启用需分别核实。[录屏分析](pixpin-screen-recording-research.md) | Lexift 继续使用已验证的 Media Foundation 录制与导出路径。 |

实时窗口贴图、贴图历史存储、快速保存和 OCR 结果窗口目前有官方行为依据。其内部采集、存储和调度方式需在对应功能启动前再调查。

## 建议实施顺序

1. **长截图采集内裁剪。** 在现有控制条加入“裁掉当前帧之前／之后”，第一版覆盖纵向手动和自动。用最后确认帧的正文范围定位，裁剪前停止自动输入；核对预览、固定栏各一份和成品像素一致。下一步滚动必须重新通过现有锚点验证。
2. **快速保存。** 先做用户主动触发的快速保存，复用冻结输出与原子写入，确定目录、命名及重名规则后再考虑自动保存。
3. **恢复已关闭的贴图。** 从托盘进入，区分已关闭贴图、存活但隐藏的贴图与成功输出的图片历史。首版可沿用进程内资源和有界容量。
4. **横向长截图。** 首版实现单屏手动向右拼接、局部预览及采集内左右裁剪。横向自动滚动和固定左右栏另行验证。
5. **窗口贴图。** Windows 已接入截图编辑的「更多」菜单，支持单路局部窗口捕获和冻结为普通贴图。[接入验证](../releases/window-pin.md) · [原生技术验证](../releases/window-pin-gate.md)
6. **截图预设、OCR 结果窗口、多窗口及异形截图。** 各自按独立链路交付。预设优先复用延迟截图和区域校验；多区域先明确透明、坐标与输出规则。
7. **录屏后处理和其他贴图类型。** 按使用场景选做，录屏历史延后。

Lexift 已接入多区域 PNG 批量保存和多窗口透明 PNG 保存。多窗口使用冻结桌面可见像素，异形选区另行设计。

Lexift 已接入逐点闭合的多边形截图，成品为透明 PNG。自由手绘和选区加减另行设计。

Lexift 已接入矩形选区加减，按操作顺序合成一张透明 PNG，并支持会话内撤销。

Lexift 已接入自由手绘截图，支持奇偶填充、重画和透明 PNG。
