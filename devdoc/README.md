# 开发文档

## 阅读入口

- 了解模块职责与依赖：[架构决策](architecture/architecture.md) → [Cargo Workspace](architecture/workspace.md)。
- 查找功能行为和实现边界：阅读下方的功能、标注与平台文档。
- 继续标注重构：[执行计划](annotation/refactoring/annotation-refactor-plan.md) → [实现与实验记录](annotation/refactoring/annotation-refactor.md)。
- 准备发布：[发布检查清单](releases/release-checklist.md) → 对应版本的发布说明。

文档中的验证结果只代表记录时的源码和环境；“待完成”“未实测”和失败实验保留原意，不代表当前版本已通过验收。

## 目录

| 目录 | 内容 |
| --- | --- |
| `architecture/` | 架构决策、crate 职责及依赖规则 |
| `planning/` | 产品能力路线和阶段实施计划 |
| `features/` | 翻译、Settings、剪贴板等功能说明 |
| `annotation/` | 标注工具、参数、窗口与交互说明 |
| `annotation/refactoring/` | 标注重构执行计划与详细记录 |
| `platform/` | Windows 输入、托盘、窗口和性能诊断 |
| `releases/` | 发布检查、测量、验收记录及版本说明 |
| 各主题下的 `archive/` | 已被后续文档替代的设计或计划 |

原有 `devdoc/archive/` 是 Git 忽略的本地参考素材目录，截图、录屏及个人笔记保持原位，与上述版本化文档归档分开。

## 架构与规划

- [架构决策](architecture/architecture.md)
- [Cargo Workspace 设计](architecture/workspace.md)
- [功能能力与路线图](planning/capabilities-roadmap.md)
- [PixPin 刷新底图研究](planning/pixpin-refresh-research.md)
- [PixPin 屏幕录制链路、性能与交互研究](planning/pixpin-screen-recording-research.md)
- [屏幕录制技术验证与重复执行](releases/screen-recording-gate.md)
- [无声 MP4 与实时标注实施及验收状态](releases/screen-recording-mp4.md)
- [录屏系统声音与麦克风实施及验收状态](releases/screen-recording-audio.md)
- [录屏音频同步、缺口修复与剩余验证](releases/recording-audio-validation.md)
- [录屏帧率、画质与性能验证](releases/recording-video-options.md)
- [录屏倒计时开始验证](releases/recording-countdown.md)
- [录屏完成后预览与回放验证](releases/recording-preview.md)
- [录屏预览同步与资源收尾专项](releases/recording-preview-validation.md)
- [PixPin 长截图链路与交互研究](planning/pixpin-long-screenshot-research.md)
- [PixPin 长图结果编辑器研究](planning/pixpin-image-editor-research.md)
- [阶段实施计划与阶段验收记录](planning/implementation-plan.md)：M2／M3／M4 的阶段背景，不能单独作为当前功能清单。

## 功能

- [主页面使用指南](features/main-usage-guide.md) · [专项验证](releases/main-usage-guide.md)

- [大模型服务与翻译设置](features/llm-providers.md)
- [翻译 Popup 会话与窗口策略](features/translation-popup.md)
- [长译文布局](features/translation-result-layout.md)
- [Settings 使用体验](features/settings-ergonomics.md)
- [本地 OCR 框选翻译](features/ocr-translation.md)
- [图片编辑器 OCR 识别复制](features/image-editor-ocr.md)
- [图片尺寸调整](features/image-resize.md)
- [屏幕区域选择基础](features/region-selection.md)
- [共用屏幕交互宿主](features/screen-interaction.md)
- [独立截图快捷键](features/screenshot-hotkey.md) · [专项验证](releases/screenshot-hotkey.md)
- [截图模式与临时贴图](features/screenshot-mode.md) · [刷新底图验证](releases/screenshot-refresh.md)
- [贴图交互、透明度与统一显隐](features/pin-interaction.md)
- [截图／长图图片历史](features/image-history.md)
- [图片历史验证](releases/screenshot-image-history.md)
- [标注撤销与重做](annotation/annotation-history.md)
- [选区历史与区域复用验证](releases/screenshot-region-history.md)
- [截图模式验证](releases/screenshot-mode-2026-10-13.md)
- [多贴图输入与生命周期验证](releases/multi-pin-interaction-2026-10-13.md)
- [剪贴板历史](features/clipboard-history.md)
- [复制按钮反馈](features/copy-feedback.md)
- [界面国际化](features/i18n.md)
- 历史参考：[Ollama 首版实现](features/archive/ollama-provider.md)。

## 标注

- [工具条拖动、收起与面板准备](annotation/annotation-toolbar-preparation.md)
- [工具选择菜单首次展开验证](releases/annotation-menu-latency-2026-10-12.md)
- [标注字体后台加载验证](releases/annotation-font-background-2026-10-12.md)
- [矩形橡皮擦连续编辑](annotation/rectangle-eraser-editing.md)
- [矩形橡皮擦验证](releases/rectangle-eraser-editing-2026-10-12.md)
- [标注按钮悬停提示](annotation/annotation-hints.md)
- [标注按钮提示验证](releases/annotation-hints-2026-10-12.md)
- [参数下拉菜单](annotation/annotation-dropdown.md)
- [数值参数 UI](annotation/annotation-numeric-ui.md)
- [自定义选色器](annotation/annotation-color-picker.md)
- [字体目录与默认选择](annotation/annotation-fonts.md)
- [铅笔与荧光笔](annotation/annotation-brushes.md)
- [序号标注](annotation/annotation-sequence.md)
- [马赛克贴纸](annotation/annotation-sticker.md)
- [光标反馈](annotation/annotation-cursor.md)
- [下载与保存](annotation/annotation-download.md)

### 标注重构

- [执行计划](annotation/refactoring/annotation-refactor-plan.md)：维护后续执行顺序和未完成范围。
- [实现与实验记录](annotation/refactoring/annotation-refactor.md)：保留迁移、测量、撤回实验及验证证据；不是并行执行计划。
- 归档：[早期实施路线](annotation/refactoring/archive/annotation-refactor-roadmap.md)、[中间轮次计划与交接](annotation/refactoring/archive/annotation-refactor-next.md)。两份旧计划中的“当前”“下一批”只表示当时状态。

## 平台与诊断

- [Windows 选择兼容性](platform/windows-selection-compatibility.md)
- [划词工具条输入隔离](platform/selection-input-isolation.md)
- [Windows 鼠标输入隔离](platform/mouse-input-isolation.md)
- [托盘菜单](platform/tray-menu.md)
- [窗口缩放性能诊断](platform/resize-performance.md)

## 发布

- [截图编辑焦点交接验证](releases/screenshot-editor-focus.md)
- [手动纵向长截图验证](releases/screenshot-long-capture.md)
- [长截图采集、预览与编辑体验重做验证](releases/screenshot-long-redesign.md)
- [长截图单行工具条与延迟提示验证](releases/screenshot-long-toolbar.md)
- [长图编辑器统一图片视口验证](releases/screenshot-image-viewport.md)
- [标注重做验证](releases/annotation-redo.md)
- [截图操作提示验证](releases/screenshot-help.md)
- [截图比例锁定验证](releases/screenshot-aspect-lock.md)
- [截图像素放大镜与取色验证](releases/screenshot-pixel-picker.md)
- [截图 OCR 识别并复制验证](releases/screenshot-ocr.md)
- [窗口自动识别与一键框选验证](releases/screenshot-window-candidates.md)
- [内部元素识别与滚轮层级验证](releases/screenshot-element-candidates.md)
- [截图首次准备优化验证](releases/screenshot-startup-performance.md)
- [截图按点识别与启动性能验证](releases/screenshot-point-query-performance.md)
- [截图紧凑尺寸面板与拖动验证](releases/screenshot-size-panel.md)
- [截图手动宽高输入验证](releases/screenshot-size-input.md)
- [截图选区键盘精确调整验证](releases/screenshot-keyboard-adjustment.md)
- [鼠标模式语义与穿透回归](releases/mouse-mode-validation.md)
- [截图选择对象模式回归](releases/screenshot-object-selection.md)
- [截图选区调整反馈验证](releases/screenshot-region-adjustment.md)
- [截图尺寸持续显示验证](releases/screenshot-size-label.md)
- [截图尺寸标签层级验证](releases/screenshot-label-order.md)
- [截图尺寸同帧呈现验证](releases/screenshot-same-frame.md)
- [贴图菜单与恢复行为验证](releases/pinned-image-menu-2026-10-14.md)
- [贴图可编辑标注验证](releases/pinned-image-annotation-2026-10-14.md)
- [贴图编辑裁剪验证](releases/pin-image-crop.md)

- [标注、窗口 Bridge 与 OCR 职责整理验证](releases/responsibility-refactor-validation.md)

- [OCR 基础包、韩文扩展与性能验证](releases/ocr-extension-validation.md)
- [OCR 自动检测验证](releases/ocr-auto-validation.md)
- [OCR Provider 与资源管理验证](releases/ocr-provider-validation.md)
- [本地 OCR 实施验证](releases/ocr-validation.md)
- [OCR 首次显示验证](releases/ocr-first-frame-validation.md)
- [发布检查清单](releases/release-checklist.md)
- [Release 测量](releases/release-performance.md)
- [v0.1.0 候选验收记录](releases/release-validation-v0.1.0.md)
- 发布说明：[v0.1.0](releases/notes/release-notes-v0.1.0.md)、[rc.13](releases/notes/release-notes-v0.0.1-rc.13.md)、[rc.12](releases/notes/release-notes-v0.0.1-rc.12.md)、[rc.11](releases/notes/release-notes-v0.0.1-rc.11.md)、[rc.10](releases/notes/release-notes-v0.0.1-rc.10.md)、[rc.9](releases/notes/release-notes-v0.0.1-rc.9.md)、[rc.8](releases/notes/release-notes-v0.0.1-rc.8.md)、[rc.7](releases/notes/release-notes-v0.0.1-rc.7.md)、[rc.6](releases/notes/release-notes-v0.0.1-rc.6.md)、[rc.5](releases/notes/release-notes-v0.0.1-rc.5.md)、[rc.4](releases/notes/release-notes-v0.0.1-rc.4.md)、[rc.3](releases/notes/release-notes-v0.0.1-rc.3.md)、[rc.2](releases/notes/release-notes-v0.0.1-rc.2.md)、[rc.1](releases/notes/release-notes-v0.0.1-rc.1.md)。

## 维护约定

- 新文档放入对应主题目录，并在本索引增加入口；文件名采用小写英文和连字符。
- 同一主题只维护一个执行计划；被替代的计划移入该主题的 `archive/`，注明后续入口。
- 功能文档说明行为和边界；验证记录注明日期、环境、失败及未覆盖范围，避免把历史结果写成当前保证。
- 文档间使用相对链接。移动文件时同步更新脚本、CI 和仓库内引用。
- 日志、截图、录屏及临时夹具继续留在忽略的 `target/`，文档只记录路径及证据范围。

- [延迟截图](features/screenshot-delay.md)
- [延迟截图验证](releases/screenshot-delay.md)

- [图片保存格式 PNG／JPEG](features/image-save-formats.md)
- [图片保存格式验证](releases/image-save-formats.md)
- [从文件／剪贴板打开图片](features/image-import.md)
- [图片导入验证](releases/image-import.md)
- [贴图透明度验证](releases/pin-opacity.md)
- [贴图统一隐藏／恢复验证](releases/pin-visibility.md)
- [贴图旋转／翻转验证](releases/pin-image-transform.md)

- [贴图倍率与编辑视口验证](releases/pin-image-zoom.md)
- [贴图灰度与反色验证](releases/pin-image-color.md)
- [托盘菜单连续打开验证](releases/tray-menu-stability.md)
- [贴图亮度调节验证](releases/pin-brightness.md)
- [贴图对比度调节验证](releases/pin-contrast.md)

- [独立图片编辑器颜色调整](features/image-editor-colors.md)
- [独立图片编辑器颜色验证](releases/image-editor-colors.md)

- [大图编辑性能与资源管理](features/large-image-performance.md)
- [大图编辑性能验证](releases/large-image-performance.md)

- [录屏首尾裁剪验证](releases/recording-trim.md)
