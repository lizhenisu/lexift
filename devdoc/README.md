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
- [阶段实施计划与阶段验收记录](planning/implementation-plan.md)：M2／M3／M4 的阶段背景，不能单独作为当前功能清单。

## 功能

- [大模型服务与翻译设置](features/llm-providers.md)
- [翻译 Popup 会话与窗口策略](features/translation-popup.md)
- [长译文布局](features/translation-result-layout.md)
- [Settings 使用体验](features/settings-ergonomics.md)
- [本地 OCR 框选翻译](features/ocr-translation.md)
- [屏幕区域选择基础](features/region-selection.md)
- [共用屏幕交互宿主](features/screen-interaction.md)
- [剪贴板历史](features/clipboard-history.md)
- [复制按钮反馈](features/copy-feedback.md)
- [界面国际化](features/i18n.md)
- 历史参考：[Ollama 首版实现](features/archive/ollama-provider.md)。

## 标注

- [工具条拖动、收起与面板准备](annotation/annotation-toolbar-preparation.md)
- [工具选择菜单首次展开验证](releases/annotation-menu-latency-2026-10-12.md)
- [标注字体后台加载验证](releases/annotation-font-background-2026-10-12.md)
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

- [标注、窗口 Bridge 与 OCR 职责整理验证](releases/responsibility-refactor-validation.md)

- [OCR 基础包、韩文扩展与性能验证](releases/ocr-extension-validation.md)
- [OCR 自动检测验证](releases/ocr-auto-validation.md)
- [OCR Provider 与资源管理验证](releases/ocr-provider-validation.md)
- [本地 OCR 实施验证](releases/ocr-validation.md)
- [OCR 首次显示验证](releases/ocr-first-frame-validation.md)
- [发布检查清单](releases/release-checklist.md)
- [Release 测量](releases/release-performance.md)
- [v0.1.0 候选验收记录](releases/release-validation-v0.1.0.md)
- 发布说明：[v0.1.0](releases/notes/release-notes-v0.1.0.md)、[rc.10](releases/notes/release-notes-v0.0.1-rc.10.md)、[rc.9](releases/notes/release-notes-v0.0.1-rc.9.md)、[rc.8](releases/notes/release-notes-v0.0.1-rc.8.md)、[rc.7](releases/notes/release-notes-v0.0.1-rc.7.md)、[rc.6](releases/notes/release-notes-v0.0.1-rc.6.md)、[rc.5](releases/notes/release-notes-v0.0.1-rc.5.md)、[rc.4](releases/notes/release-notes-v0.0.1-rc.4.md)、[rc.3](releases/notes/release-notes-v0.0.1-rc.3.md)、[rc.2](releases/notes/release-notes-v0.0.1-rc.2.md)、[rc.1](releases/notes/release-notes-v0.0.1-rc.1.md)。

## 维护约定

- 新文档放入对应主题目录，并在本索引增加入口；文件名采用小写英文和连字符。
- 同一主题只维护一个执行计划；被替代的计划移入该主题的 `archive/`，注明后续入口。
- 功能文档说明行为和边界；验证记录注明日期、环境、失败及未覆盖范围，避免把历史结果写成当前保证。
- 文档间使用相对链接。移动文件时同步更新脚本、CI 和仓库内引用。
- 日志、截图、录屏及临时夹具继续留在忽略的 `target/`，文档只记录路径及证据范围。
