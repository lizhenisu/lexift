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

项目采用 Virtual Cargo Workspace，当前保持 7 个核心 crate：

- `lexift-app`：程序入口与 Composition Root，只负责初始化、生命周期和模块组装，不承载领域业务逻辑。
- `lexift-core`：核心领域模型、状态、事件、命令、用例和 Port 接口；是唯一业务核心。
- `lexift-ui`：Slint UI、UI bridge、状态映射和交互事件转发。
- `lexift-platform`：原生系统能力适配，包括划词、剪贴板、全局快捷键、屏幕、托盘、权限、安全凭证等；当前交付以 Windows 为主，macOS/Linux 为后续适配目标。
- `lexift-translate`：翻译 Provider、网络调用、Provider Registry、请求/响应适配。
- `lexift-config`：普通配置的读取、写入、默认值、Schema 与迁移；敏感凭证不直接明文存入普通配置文件。
- `lexift-observability`：日志初始化、panic hook、崩溃诊断和性能观测。

文档入口见 [开发文档索引](devdoc/README.md)，详细目录和职责见 [Workspace 设计](devdoc/architecture/workspace.md)。历史计划和验证记录不能替代当前源码状态。

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
- **UI renders state**：`lexift-ui` 展示领域状态、管理视图与临时交互状态，并转发领域事件；业务规则由 Core 决定，不直接调用操作系统 API 或翻译 Provider。
- **App wires everything together**：`lexift-app` 创建并注入具体实现，调度 Core 命令并回传完成事件；不能把领域规则放入 App。
- 业务能力使用 Core 定义的 Port；原生窗口准备、定位、拖动、重绘等视图桥接可由 App 以回调注入 UI，具体平台实现仍在 Platform。不得为纯窗口接线强行新增 Core Port，也不得通过回调绕过 Core 执行业务。
- 标注导出由 UI 冻结可见文档，Core 管理请求、取消和结果，App 调度独立采集、纯图像合成及保存；UI 不创建导出线程、轮询采集或调用保存 Port。纯合成实现可复用 UI 软件渲染，但不能读取活视图或调用原生系统能力。

### 线程与异步结果

- Slint 组件和属性在 UI 线程访问；网络、磁盘及可能阻塞的平台调用放在后台。具有消息循环、COM apartment 或线程亲和性的平台能力由 Platform 管理所属线程，不任意迁移到通用 worker。
- 后台领域结果先转为 Core 事件，UI 更新投递到 Slint 事件循环；不在持有共享状态锁时调用 UI 或外部回调。
- 窗口异步操作校验窗口实例、会话和请求版本；目标相关操作同时校验目标身份。关闭、切换目标、取消或替换任务后，旧结果不得更新新窗口或操作新目标。

### 平台代码约束

- `#[cfg(target_os = "...")]` 等平台条件编译应尽量限制在 `lexift-platform` 内，禁止无必要地散落到 Core、UI 或 Translate。
- 各平台实现遵守相同的 Core Port 契约；尚未支持的能力明确返回不可用，不能以空实现假报成功。实现 Linux 能力时区分 X11 与 Wayland。
- 不允许在 `lexift-platform` 中实现翻译业务，在 `lexift-translate` 中实现系统剪贴板、快捷键、窗口等平台逻辑。

### UI 约束

- `.slint` 文件归 `lexift-ui` 管理；Slint 的编译与 `build.rs` 也由 `lexift-ui` 自己负责。
- UI 不依赖 `lexift-platform`、`lexift-translate` 或具体 Provider；领域操作经过 Core，原生视图接线遵循上述 App 注入边界。
- 公共视觉值使用统一 Design Tokens（颜色、字号、间距、圆角、阴影等），避免界面中散落无规则 magic numbers。
- 新增或修改可见文案、提示及可访问名称时，同步现有 12 种界面语言；沿用已有国际化机制。

### Windows 窗口策略

- 仅 Settings 允许进入任务栏和 Alt+Tab；主窗口、Popup、工具条、编辑器、画布、各级菜单及软件发起的原生对话框均使用辅助窗口策略。
- 按窗口实例和角色设置策略，隐藏准备完成后才显示；准备失败使用既有有界重试，耗尽后销毁未显示窗口。平台持续保护窗口角色，点击、固定、拖动、缩放或后端样式重写不得改变身份。
- 任务栏排除与激活、输入能力独立；保留用户主动操作所需焦点，不通过事后抢焦点、显示后隐藏或持续轮询修补窗口身份。实现依据见 [Popup 与窗口策略](devdoc/features/translation-popup.md)。

### 窗口与内存生命周期

- 新窗口按需创建；关闭窗口时销毁对应 Slint 组件和原生窗口，并清理该窗口持有的事件回调、平台钩子、临时 UI 状态及延迟任务。不要为方便重开而长期保留隐藏窗口。
- 窗口重开时从最新 Core 状态重建 UI。后台进程确需继续运行时，应保留托盘、快捷键和事件循环，不因关闭普通窗口退出。
- 最后一个 UI 窗口销毁后，使用可取消的两分钟空闲计时；计时期间任一窗口重开都要取消计时。到期后的工作集修剪通过 `lexift-platform` 提供、由 `lexift-app` 注入 UI 生命周期，不允许 UI 直接调用平台 API。
- 工作集修剪只回收驻留页，不等同于释放 PrivateBytes 或重建 Slint/Winit 后端。Renderer、surface、GPU 资源应随所属窗口销毁；新窗口按需创建并重建其 Renderer。新增窗口或后台能力时须遵循并验证这套生命周期，不能依赖工作集修剪替代资源所有权清理。
- 托盘、快捷键、剪贴板监听等后台资源由运行时拥有；退出或关闭对应能力时撤销监听、取消任务并完成必要的持久化收尾，不绑定到已销毁的视图。

### 配置与安全

- 普通设置由 `lexift-config` 管理。
- API Key、Token 等敏感信息通过 Core 的 `CredentialStore` Port 抽象，由 `lexift-platform` 对接 Windows Credential Manager、macOS Keychain、Linux Secret Service 等系统安全存储。
- 普通配置文件只保存 Provider 标识、credential id 等非敏感引用，不保存明文密钥。
- 设置变更在最新配置上定向合并并持久化，避免旧页面快照覆盖其他已保存字段；草稿与已保存状态分离，失败保留可恢复状态并反馈。
- 剪贴板历史正文属于敏感数据，使用现有独立加密存储，不写入普通配置、日志或 Debug 正文；凭证复制保留历史排除标记，解密失败不覆盖原文件或回退明文。

### 验证与文档

- 按改动影响运行相关 crate 回归、格式和 Clippy 检查；Slint 修改必须通过 UI 编译。涉及 Windows 原生窗口或输入时，补充对应实机验证，区分自动化、实机结果及未覆盖范围。
- 涉及发布构建或跨 crate 接口时运行相应 Release 构建；纯文档整理检查链接、引用和脚本路径即可，无需无关全量构建。
- 功能或架构变化同步对应主题文档；执行计划与历史证据分开维护，日志、截图和临时夹具放入忽略目录，不将测试正文或个人数据纳入提交。
- Python `__pycache__`、`.pyc`、`.pyo` 不纳入版本控制。模型资源包、下载副本、Python 虚拟环境、截图／录屏、性能记录与临时夹具放入忽略的 `target/`，发布产物放入 `dist/`；运行所需资源清单、许可及经审核的聚合校准参数仍按源码管理。

### 扩展原则

- 后续版本只有当某功能形成独立复杂领域、拥有清晰公共接口并显著降低现有 crate 内聚性时，才新增 crate。
- 新增 crate 前必须检查依赖 DAG，禁止引入循环依赖。
