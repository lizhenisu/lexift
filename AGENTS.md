# 仓库指南

## 通用级

- **任务范围与完成标准**：在当前任务范围内完成实现和必要验证，并修复本次改动引入的问题，无需在常规步骤间重复确认。验证范围与改动影响相匹配；无关问题记录后说明，不主动扩大重构或测试范围。

- **模块化设计**：项目采用模块化架构，确保各功能单元高内聚、低耦合，便于独立开发、测试、模块替换、后续功能扩展。新增代码按功能领域放入对应子包，并暴露必要的公共接口。

- **通用逻辑优先**：多个模块存在清晰的复用边界时，优先抽象为领域工具、基础组件或纯函数，避免复制实现。

- **文档与注释**：对复杂或抽象的函数、类、模块添加简洁的 docstring，说明用途、关键参数与返回值。注释解释“为什么”，而非重复描述代码“做什么”；简单逻辑优先用清晰命名、合理拆分和类型表达。

### Git

- 不提交 `.env`、令牌、凭证、个人数据或生成的数据集/检查点。

- **未经用户明确授权，禁止暂存、提交、推送、覆盖远端或重写历史**。用户要求“提交暂存区”时，只提交已经 staged 的文件；不得自行追加未暂存改动。

- 禁止未经授权的破坏性清理或删除用户已有改动；任务所需的代码删改、文件替换及本次生成的临时文件清理可直接执行，但必须保留与本次任务无关的用户已有改动。

### UI、交互与国际化

- UI 以既有组件和 Design Tokens 为准。新增组件或界面优先复用既有设计、设置卡片、输入控件和弹层机制，只有无法满足需求时才新增样式体系。

- 同类交互保持一致；仅调整本次任务涉及的界面。

- 焦点策略：应用窗口、弹窗及其他 UI 非必要不主动抢占系统焦点。
- 图标：图标用svg，而不是字体

### 日志

- 日志用于帮助开发者定位程序 bug、崩溃等问题，应避免明显影响交互响应；热路径避免昂贵计算和高频输出。

## 项目级

### Cargo Workspace

项目采用 Virtual Cargo Workspace，V0.1 固定为 7 个核心 crate：

- `lexift-app`：程序入口与 Composition Root，只负责初始化、生命周期和模块组装，不承载领域业务逻辑。
- `lexift-core`：核心领域模型、状态、事件、命令、用例和 Port 接口；是唯一业务核心。
- `lexift-ui`：Slint UI、UI bridge、状态映射和交互事件转发。
- `lexift-platform`：Windows/macOS/Linux 原生系统能力适配，包括划词、剪贴板、全局快捷键、屏幕、托盘、权限、安全凭证等。
- `lexift-translate`：翻译 Provider、网络调用、Provider Registry、请求/响应适配。
- `lexift-config`：普通配置的读取、写入、默认值、Schema 与迁移；敏感凭证不直接明文存入普通配置文件。
- `lexift-observability`：日志初始化、panic hook、崩溃诊断和性能观测。

详细目录和职责见 `devdoc/workspace.md`。

### 依赖方向

- `lexift-core` **不得依赖任何其他 Lexift crate**。
- 在 Lexift 内部 crate 中，`lexift-ui`、`lexift-platform`、`lexift-translate`、`lexift-config` 只能依赖 `lexift-core`，外围 Adapter 之间禁止直接形成业务依赖。
- `lexift-observability` 应保持独立，不作为所有 crate 的强制内部依赖；其他 crate 可直接使用轻量 `tracing` API，由 `lexift-app` 统一初始化观测系统。
- `lexift-app` 是唯一 Composition Root，可以依赖并组装全部核心 crate。
- 禁止 `core -> ui/platform/translate/config`、`ui -> platform/translate`、`platform -> translate`、`translate -> platform` 等反向或横向依赖。

### Core / Port / Adapter 原则

- **Core defines behavior**：业务行为、领域模型、状态机和能力接口由 `lexift-core` 定义。
- **Platform provides capabilities**：操作系统能力由 `lexift-platform` 实现 Core 定义的 Port。
- **Translate provides services**：翻译能力由 `lexift-translate` 实现 Core 定义的翻译 Port。
- **UI renders state**：`lexift-ui` 只展示 Core 状态并把用户操作转为 Core 可理解的事件，不直接调用平台 API 或翻译 Provider。
- **App wires everything together**：`lexift-app` 只负责创建具体实现并注入 Core。

### 平台代码约束

- `#[cfg(target_os = "...")]` 等平台条件编译应尽量限制在 `lexift-platform` 内，禁止无必要地散落到 Core、UI 或 Translate。
- Windows、macOS、Linux 的实现必须遵守同一组 Core Port；Linux 内部应明确区分 X11 与 Wayland。
- 不允许在 `lexift-platform` 中实现翻译业务，在 `lexift-translate` 中实现系统剪贴板、快捷键、窗口等平台逻辑。

### UI 约束

- `.slint` 文件归 `lexift-ui` 管理；Slint 的编译与 `build.rs` 也由 `lexift-ui` 自己负责。
- UI 不得直接访问 `lexift-platform`、`lexift-translate` 或 Provider；所有业务动作必须经过 Core。
- 公共视觉值使用统一 Design Tokens（颜色、字号、间距、圆角、阴影等），避免界面中散落无规则 magic numbers。

### 窗口与内存生命周期

- 新窗口按需创建；关闭窗口时销毁对应 Slint 组件和原生窗口，并清理该窗口持有的事件回调、平台钩子、临时 UI 状态及延迟任务。不要为方便重开而长期保留隐藏窗口。
- 窗口重开时从最新 Core 状态重建 UI。后台进程确需继续运行时，应保留托盘、快捷键和事件循环，不因关闭普通窗口退出。
- 最后一个 UI 窗口销毁后，使用可取消的两分钟空闲计时；计时期间任一窗口重开都要取消计时。到期后的工作集修剪通过 `lexift-platform` 提供、由 `lexift-app` 注入 UI 生命周期，不允许 UI 直接调用平台 API。
- 工作集修剪只回收驻留页，不等同于释放 PrivateBytes 或重建 Slint/Winit 后端。Renderer、surface、GPU 资源应随所属窗口销毁；新窗口按需创建并重建其 Renderer。新增窗口或后台能力时须遵循并验证这套生命周期，不能依赖工作集修剪替代资源所有权清理。

### 配置与安全

- 普通设置由 `lexift-config` 管理。
- API Key、Token 等敏感信息通过 Core 的 `CredentialStore` Port 抽象，由 `lexift-platform` 对接 Windows Credential Manager、macOS Keychain、Linux Secret Service 等系统安全存储。
- 普通配置文件只保存 Provider 标识、credential id 等非敏感引用，不保存明文密钥。

### 扩展原则

- 后续版本只有当某功能形成独立复杂领域、拥有清晰公共接口并显著降低现有 crate 内聚性时，才新增 crate。
- 新增 crate 前必须检查依赖 DAG，禁止引入循环依赖。
