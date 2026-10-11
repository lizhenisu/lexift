# Lexift Cargo Workspace 设计

本文档定义 Lexift V0.1 的实际 Cargo Workspace 结构、7 个核心 crate 的职责边界、依赖方向，以及后续扩展时必须遵守的架构约束。

## 1. 总体原则

Lexift 使用 **Virtual Cargo Workspace + Dependency Inversion**。

核心规则：

> Core defines behavior, Platform provides capabilities, Translate provides services, UI renders state, App wires everything together.

对应中文：

> Core 定义行为，Platform 提供系统能力，Translate 提供翻译能力，UI 只负责状态展示，App 负责组装。

`lexift-core` 是唯一业务核心。平台、翻译、配置和 UI 都位于 Core 外围，通过 Core 定义的 Port/trait 协作。

屏幕交互的公共边界见 [共用屏幕交互宿主](../features/screen-interaction.md)：UI 管理实例、输入队列及视图任务，Platform 提供隐藏创建和原生准备，App 注入窗口回调；Core 仅提供区域手势与快照裁切。标注与 OCR 保持独立业务会话和宿主实例。

## 2. 实际目录结构

以下列出当前职责入口；不是包含所有源文件的展开清单。

```text
lexift/
├── Cargo.toml / Cargo.lock / AGENTS.md
├── devdoc/
│   ├── architecture/      # 架构及依赖约束
│   ├── features/          # 当前功能说明
│   ├── annotation/        # 标注功能及重构记录
│   ├── platform/          # 原生能力和诊断
│   ├── planning/          # 阶段计划
│   └── releases/          # 验证与发布记录
├── assets/ / packaging/ / scripts/
└── crates/
    ├── lexift-app/src/
    │   ├── bootstrap.rs / wiring.rs / controller.rs
    │   ├── annotation_export.rs    # 导出采集、合成、保存后台任务
    │   ├── ocr.rs / clipboard_history.rs
    │   └── runtime/                # 快捷键与翻译能力生命周期
    ├── lexift-core/src/
    │   ├── domain/                 # 标注、导出、OCR、历史及翻译状态
    │   ├── ports/                  # 能力契约，包括纯导出合成
    │   └── event.rs / command.rs / state.rs
    ├── lexift-ui/
    │   ├── src/annotation/         # 会话、画布、输入、参数、编辑与渲染
    │   ├── src/bridge/             # 按窗口拆分的 UI 接线及生命周期
    │   ├── src/ocr/                # selection 框选与 provider 设置桥接
    │   ├── src/screen_interaction/ # 共用屏幕交互宿主
    │   ├── ui/ / translations/
    │   └── build.rs
    ├── lexift-platform/src/
    │   ├── windows/ / macos/ / linux/
    │   └── ocr/                    # 本地资源、模型路由、CPU 推理和候选评分
    ├── lexift-translate/src/
    │   ├── registry.rs
    │   └── providers/              # DeepL、Ollama 及 LLM 协议适配等
    ├── lexift-config/src/          # Schema、存储及迁移
    └── lexift-observability/src/   # 日志、panic 与诊断初始化
```

根目录 `packager.toml`、`packaging/windows/installer.nsi` 与 `scripts/package-release.ps1` 负责 Windows 安装器、portable ZIP 和校验文件。模型包、Python 环境、截图和临时验证产物放在忽略的 `target/`；发布产物放在 `dist/`，不作为源码提交。

### 2.1 标注和窗口模块边界

- 标注入口负责初始化、状态同步和生命周期入口，Registry 组合画布、工具条、渲染、编辑和参数状态。输入、文字／焦点、面板、参数和呈现工作流各自维护，测试按职责分组。
- Bridge 保留稳定的 `Ui`／`UiHandle` 接口和公共窗口契约；Popup、语言菜单、主窗口、Settings、历史和划词工具条负责各自接线与清理。主窗口和 Settings 共用的隐藏准备由 `app_window` 协调。
- 导出由 UI 冻结可见文档，Core 接受带会话／请求编号的事件并产生命令，App 使用独立采集实例执行后台合成及保存，完成结果再经过 Core。UI 的纯软件合成器实现 Core `AnnotationExportRenderPort`，不访问 Slint 组件或原生能力。
- OCR `selection` 只管理屏幕宿主；`provider` 只负责设置和资源状态展示。Core 策略描述自动／含韩文／指定语言，具体模型名称、版本及兼容性归 Platform。当前仍保持 7 个 crate，不单独抽出 OCR crate。

## 3. 根 Workspace

根 `Cargo.toml` 只作为 Virtual Workspace，不同时承担 package 职责。

建议结构：

```toml
[workspace]
resolver = "3"
members = [
    "crates/lexift-app",
    "crates/lexift-core",
    "crates/lexift-ui",
    "crates/lexift-platform",
    "crates/lexift-translate",
    "crates/lexift-config",
    "crates/lexift-observability",
]

default-members = [
    "crates/lexift-app",
]

[workspace.package]
edition = "2024"
version = "0.0.1-rc.9"
```

内部 crate 和常用第三方依赖统一通过 `[workspace.dependencies]` 管理，成员 crate 使用 `.workspace = true` 继承，减少版本漂移和重复声明。

## 4. 七个核心 crate 的职责

### 4.1 `lexift-app`

定位：**程序入口 + Composition Root**。

职责：

- 初始化日志和性能观测。
- 加载配置。
- 创建平台实现。
- 创建翻译 Provider Registry。
- 创建 Core 应用实例。
- 创建并启动 UI。
- 应用持久化设置到当前 Hotkey 与 Translator Runtime，并负责失败回滚。
- 管理程序生命周期和退出流程。

禁止：

- 承载翻译领域逻辑。
- 直接实现 Provider。
- 直接实现 Win32/macOS/Wayland 具体能力。
- 把复杂业务状态放入 `main.rs`。

`main.rs` 应保持轻量，真正的组装逻辑放入 `bootstrap.rs` / `wiring.rs`；动态 Adapter 生命周期与
原子切换由 `runtime/` 管理，Controller 只执行 Core 发出的 Runtime Apply 命令。

### 4.2 `lexift-core`

定位：**唯一业务核心**。

职责：

- 领域模型：Selection、Language、TranslateRequest、TranslateResult、Settings、RuntimeConfig 等。
- 应用状态：AppState、TranslationState、PopupState 等。
- 业务事件与命令：AppEvent、AppCommand。
- Use Case：划词翻译、输入翻译、剪贴板翻译等。
- Port/trait：SelectionPort、ClipboardPort、HotkeyPort、TranslatorPort、SettingsStore、CredentialStore 等。
- 统一领域错误模型。

核心约束：

- 不知道 Slint。
- 不知道 Windows/macOS/Linux。
- 不依赖具体 Provider 实现、协议客户端或推理 SDK；可定义稳定 Provider 身份及业务能力。
- 不知道 TOML 或具体配置文件格式。
- 不依赖任何其他 Lexift crate。

### 4.3 `lexift-ui`

定位：**Slint 表现层**。

职责：

- `.slint` 界面和组件。
- Design Tokens：颜色、字号、间距、圆角、阴影、主题。
- UI 状态和 Core 状态之间的映射。
- 将按钮、输入、选择等用户操作转换为 Core 可理解的事件。
- 将 Core 状态变化呈现为 UI。

约束：

- 只能依赖 `lexift-core`。
- 不直接访问 Clipboard、Hotkey、Screen 等平台 API。
- 不直接调用 DeepL、Google、OpenAI 等 Provider。
- Slint 的 `build.rs` 和 UI 编译过程由本 crate 自己管理。

推荐结构：

```text
ui/
├── main.slint
├── popup.slint
├── settings.slint
├── components/
└── design/
    ├── colors.slint
    ├── typography.slint
    ├── spacing.slint
    └── theme.slint
```

### 4.4 `lexift-platform`

定位：**操作系统能力 Adapter**。

职责：

- 获取当前选中文字。
- 剪贴板读取和写入。
- 全局快捷键。
- 屏幕、光标和窗口位置。
- 截图。
- 系统托盘。
- 开机启动。
- 权限检查与引导。
- 系统级安全凭证存储。
- 当前本地 PaddleOCR 资源下载、完整校验、删除、模型路由与 CPU 推理；与原生屏幕采集通过 Core 契约分隔。

平台拆分：

```text
Windows
macOS
Linux
├── X11
└── Wayland
```

约束：

- 实现 Core 定义的 Port。
- 平台条件编译尽量限制在本 crate 内。
- 不实现翻译业务。
- 不依赖 `lexift-ui` 或 `lexift-translate`。

### 4.5 `lexift-translate`

定位：**翻译服务 Adapter**。

职责：

- 实现 Core 定义的 `TranslatorPort`。
- HTTP 请求和连接复用。
- API 鉴权和请求格式转换。
- Provider 响应解析。
- 错误映射。
- Provider Registry。
- 以请求快照配置执行翻译，支持当前 LLM 的正文增量与完整结果。

当前设置中的翻译 Provider 为 DeepL、Ollama、LM Studio、DeepSeek。目录中的 Google、OpenAI、Gemini 实现或协议代码不等于设置中已开放的服务。OCR Provider 独立于翻译模型选择器。

约束：

- 不知道 Slint。
- 不访问系统剪贴板、快捷键或窗口 API。
- 不依赖 `lexift-platform`。

### 4.6 `lexift-config`

定位：**普通配置持久化 Adapter**。

职责：

- 配置 Schema。
- 默认值。
- 配置加载和写入。
- Schema Version。
- 配置迁移。
- 配置文件路径。

例如普通配置可包含：

```toml
[translation]
target_language = "zh-CN"

[ui]
theme = "system"

[hotkey]
translate = "Alt+X"
```

安全边界：

- API Key、Token 不明文保存在普通配置中。
- 配置中只保存 `credential_id` 等非敏感引用。
- 真正的 Secret 存储通过 Core 的 `CredentialStore` Port，由 `lexift-platform` 对接系统安全存储。

### 4.7 `lexift-observability`

定位：**观测系统初始化与诊断基础设施**。

职责：

- `tracing` subscriber 初始化。
- 日志输出策略。
- panic hook。
- 崩溃诊断。
- 性能阶段耗时统计。
- Debug diagnostics。

约束：

- 不承载领域业务。
- 不成为所有 crate 的强制内部依赖。
- 其他 crate 可以直接调用轻量 `tracing` 宏，由 `lexift-app` 统一调用 `lexift-observability::init()`。

## 5. Port / Adapter 模型

Core 只声明能力，不知道具体实现。

例如：

```rust
pub trait SelectionPort {
    fn selected_text(&self) -> Result<Option<Selection>>;
}

pub trait ClipboardPort {
    fn read_text(&self) -> Result<Option<String>>;
    fn write_text(&self, text: &str) -> Result<()>;
}

pub trait TranslatorPort {
    async fn translate(
        &self,
        request: TranslateRequest,
    ) -> Result<TranslateResult>;
}
```

外围模块负责实现：

```text
SelectionPort
    ↑
lexift-platform

TranslatorPort
    ↑
lexift-translate

SettingsStore
    ↑
lexift-config
```

Core 因此可以在不启动 GUI、不调用真实系统 API、不访问真实翻译服务的情况下进行单元测试。

## 6. 依赖 DAG

允许的核心依赖关系：

```text
                         lexift-app
       ┌────────────┬────────────┬────────────┬────────────┬────────────────────┐
       ▼            ▼            ▼            ▼            ▼
  lexift-ui  lexift-platform lexift-translate lexift-config lexift-observability
       │            │            │            │
       └────────────┴────────────┴────────────┘
                              │
                              ▼
                         lexift-core
```

其中 `lexift-observability` 保持独立，由 `lexift-app` 负责初始化；`lexift-ui`、`lexift-platform`、`lexift-translate`、`lexift-config` 都只向 `lexift-core` 收敛，外围 Adapter 之间互不依赖。

更严格地说：

| crate | 允许依赖的内部 crate |
|---|---|
| `lexift-core` | 无 |
| `lexift-ui` | `lexift-core` |
| `lexift-platform` | `lexift-core` |
| `lexift-translate` | `lexift-core` |
| `lexift-config` | `lexift-core` |
| `lexift-observability` | 无 |
| `lexift-app` | 全部 |

注意：`lexift-platform` 与 `lexift-config` 在架构上是并列 Adapter。安全凭证由 Platform 负责，普通配置由 Config 负责；二者不应通过直接 crate 依赖耦合。

## 7. 禁止依赖

以下依赖应视为架构违规：

```text
core      -> ui
core      -> platform
core      -> translate
core      -> config

ui        -> platform
ui        -> translate
ui        -> config

platform  -> translate
platform  -> config
translate -> platform
translate -> config

config    -> ui
config    -> platform
config    -> translate
```

如果确实出现跨模块协作需求，应优先检查是否需要：

1. 在 Core 中新增领域模型。
2. 在 Core 中新增 Port。
3. 由 `lexift-app` 完成具体实现的组装。

而不是让外围 crate 直接互相调用。

## 8. 划词翻译的数据流

V0.1 最核心流程：

```text
OS
│
│ Alt + X
▼
lexift-platform
│
│ HotkeyEvent
▼
lexift-core
│
│ SelectionPort
▼
lexift-platform
│
│ Selection
▼
lexift-core
│
│ TranslateRequest
▼
lexift-translate
│
│ TranslateResult
▼
lexift-core
│
│ AppState changed
▼
lexift-ui
│
▼
翻译 Popup
```

该数据流中 UI 不直接操作 Platform，Platform 不直接调用 Translate，所有业务编排都经过 Core。

## 9. 后续扩展规则

V0.1 不提前创建大量 crate。

当前 7 个 crate 保持稳定，OCR、TTS、插件等能力先通过 Port/trait 预留边界。

只有满足以下条件时才新增 crate：

- 已形成独立复杂领域。
- 有稳定而清晰的公共接口。
- 拆分可以显著提高现有模块内聚性。
- 不会引入循环依赖。

未来可能出现：

```text
lexift-ocr
lexift-speech
lexift-plugin-api
lexift-plugin-runtime
```

但这些不属于 V0.1 的初始 Workspace。

## 10. V0.1 架构验收标准

Workspace 初始化后至少满足：

- 7 个 crate 可以独立编译。
- `lexift-core` 无任何其他 Lexift crate 依赖。
- `lexift-ui` 可以独立编译 Slint UI。
- 平台代码只存在于 `lexift-platform`。
- Provider 代码只存在于 `lexift-translate`。
- 普通配置逻辑只存在于 `lexift-config`。
- App 可以完成全部具体实现的组装。
- 不存在内部循环依赖。
- 主链路可以自然承载“选中文字 → 快捷键 → 获取 Selection → 翻译 → Popup 展示”。
