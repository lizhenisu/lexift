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

## Workspace 架构基线

Lexift V0.1 采用 **Virtual Cargo Workspace + Dependency Inversion**，固定 7 个核心 crate：

- `lexift-app`
- `lexift-core`
- `lexift-ui`
- `lexift-platform`
- `lexift-translate`
- `lexift-config`
- `lexift-observability`

架构核心原则：

> Core defines behavior, Platform provides capabilities, Translate provides services, UI renders state, App wires everything together.

其中：

- `lexift-core` 是唯一业务核心，不依赖任何其他 Lexift crate。
- `lexift-ui`、`lexift-platform`、`lexift-translate`、`lexift-config` 都是 Core 外围 Adapter，只向 Core 收敛，彼此之间不形成业务依赖。
- `lexift-observability` 保持独立，由 `lexift-app` 统一初始化。
- `lexift-app` 是唯一 Composition Root，负责创建并组装具体实现，不承载领域业务逻辑。
- 平台条件编译应尽量收敛到 `lexift-platform`。
- UI 不直接调用平台 API 或翻译 Provider；跨模块业务编排统一经过 Core。

完整目录结构、Port/Adapter 设计、依赖 DAG、禁止依赖和 V0.1 架构验收标准见 [`workspace.md`](./workspace.md)。

## 文档职责

- [`architecture.md`](./architecture.md)：记录已经确定的高层技术与架构决策。
- [`workspace.md`](./workspace.md)：记录 Cargo Workspace、crate 职责、依赖方向和代码边界。
- [`capabilities-roadmap.md`](./capabilities-roadmap.md)：记录 Lexift 可实现能力、与 Pot 的功能映射和版本路线图。

> 尚未最终确定的技术选型（异步运行时、网络库、具体 OCR 实现、插件 Runtime、各平台 API 的具体绑定方案等）不应提前写死；待后续讨论形成明确决策后再更新对应文档。
