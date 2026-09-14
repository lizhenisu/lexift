# Lexift 架构决策

## GUI 技术基线

当前已确定 Lexift 的 GUI 技术栈为：

- **语言：Rust**
- **GUI：Slint**
- **渲染与窗口：采用 Slint 的原生桌面后端，不使用 WebView**
- **目标平台：Windows、macOS、Linux**

## 设计目标

Lexift 的桌面端实现以以下目标为优先级：

1. **极致性能**：尽量降低启动延迟、交互延迟、空闲资源占用和常驻开销。
2. **原生体验**：不依赖 Electron、Chromium、WebView2、WKWebView 或其他 Web Runtime。
3. **高质量 UI**：在保持性能的前提下，使用 Slint 构建现代、统一、美观的跨平台界面。
4. **跨平台**：核心业务逻辑保持平台无关；平台相关能力通过独立适配层实现。
5. **Rust 优先**：应用业务逻辑、状态管理、网络、并发、配置及平台抽象均优先使用 Rust 实现。

## 当前已确定的原则

- 不采用 Electron。
- 不采用 Tauri + WebView 作为 GUI 方案。
- 不使用 HTML / CSS / JavaScript 作为桌面 UI 技术栈。
- GUI 使用 **Slint**。
- 核心实现使用 **Rust**。
- Windows、macOS、Linux 的平台差异应封装在平台适配层中，避免平台判断散落在业务代码中。

> 其他技术选型（异步运行时、网络库、配置系统、日志系统、OCR、插件架构、平台 API 封装方式等）尚未最终确定，应在后续架构讨论后再写入本文档。
