# Lexift 当前阶段实施计划

## 1. 当前状态

Lexift 已完成 **M2 — Real Input Translation**。

当前已经具备：

- Cargo Workspace + 7 个核心 crate
- Rust + Slint 原生 GUI
- Core / Port / Adapter 架构
- Tokio 后台任务模型
- Slint 主线程事件循环
- `Idle → Capturing → Translating → Success / Error` 状态机
- Mock `SelectionPort`
- Mock `TranslatorPort`
- `AppEvent → AppState → AppCommand` 业务流
- 后台翻译任务完成后回到 Slint UI
- Translation Popup
- M1 纵向链路测试
- DeepL Official API Adapter
- 不访问公网的 HTTP Adapter 集成测试
- GitHub CI

当前项目已经从“架构骨架阶段”进入“真实产品能力实现阶段”。

当前总体目标：

> **优先完成 Windows 上可实际使用的 Translate Everywhere V0.1。**

现阶段不扩展 OCR、TTS、插件、macOS 和 Linux，避免同时引入过多不确定性。

---

## 2. 当前开发路线

```text
M1  Architecture + Mock Vertical Slice
 ✅ 已完成
        │
        ▼
M2  Real Input Translation
 ✅ 已完成
        │
        ▼
M3  Windows Global Selection
 ← 当前阶段
        │
        ▼
M4  Desktop Integration
        │
        ▼
V0.1
可实际日用的 Windows 划词翻译版本
```

---

## 3. M2 — Real Input Translation

### 目标

将当前 Mock Translation 链路替换为真实翻译链路：

```text
用户输入文本
     ↓
选择目标语言
     ↓
Translate
     ↓
Core
     ↓
TranslatorPort
     ↓
真实 HTTP Provider
     ↓
翻译结果
     ↓
Core State
     ↓
Slint UI
```

M2 的核心目的不是完成“完整翻译软件”，而是独立验证：

> **UI → Core → Async Runtime → HTTP → Provider → Core → UI**

这条真实网络链路。

### M2.1 清理 M1 Mock 边界

状态：✅ 已完成

当前 Mock 不再作为默认 Production Adapter。

调整目标：

```text
lexift-platform
├── production
└── mock

lexift-translate
├── providers
└── mock
```

要求：

- `MockSelectionPort` 只服务测试 / 开发模式
- `MockTranslator` 只服务测试 / 开发模式
- `PlatformCapabilities::new()` 不再隐式等价于 Mock Platform
- `ProviderRegistry::new()` 不再隐式注册 Mock 为生产默认 Provider
- Composition Root 明确决定使用真实实现还是 Mock

当前实现通过 Cargo feature `m1-demo` 显式启用 Mock；默认构建使用 Production 容器，不会静默回退到 Mock。Selection 作为可选 Capability，在对应操作触发时才检查；尚未配置真实 Translator 时由非 Mock 的错误 Adapter 拒绝翻译，因此不会阻止应用启动或产生假译文。

#### M2.1.1 Optional Selection Capability

状态：✅ 已完成

`SelectionPort` 是按需 Capability。缺少 Selection 只会使划词操作进入 Error 状态，不影响应用启动和后续输入翻译链路。

### M2.2 引入 Translation Task ID

状态：✅ 已完成

增加翻译任务身份：

```rust
TranslationTaskId
```

事件逐渐调整为类似：

```rust
TranslationStarted(task_id)

TranslationFinished {
    task_id,
    result,
}

TranslationFailed {
    task_id,
    error,
}
```

Core 保存：

```text
current_translation_task
```

只接受：

```text
result.task_id == current_translation_task
```

的结果。

目的：防止快速输入时出现旧请求覆盖新请求。

```text
Request A
Request B
Request C

C 先返回
↓
显示 C

A 后返回
↓
不得覆盖 C
```

这个机制应在引入真实网络请求之前完成。

### M2.3 Settings 真正进入 Core

状态：✅ 已完成

当前已经存在：

```text
Settings.target_language
```

但 reducer 仍硬编码：

```text
zh-CN
```

需要调整为：

```text
Config
 ↓
lexift-app
 ↓
Settings
 ↓
Core / AppState
 ↓
TranslateRequest
```

要求：

- 删除 reducer 中目标语言硬编码
- `AppState` 或应用上下文能够获取当前 Settings
- TranslateRequest 从当前用户设置获取目标语言
- 后续 UI 修改目标语言时无需修改 Provider 实现

当前实现由 Composition Root 将 `AppConfig.settings` 注入 `AppState`，Core 创建
`TranslateRequest` 时读取当前 `Settings.target_language`，不再硬编码目标语言。

### M2.4 实现输入翻译

状态：✅ 已完成

新增一条不经过 Selection 的业务路径：

```text
InputTranslationRequested
        │
        ▼
TranslateRequest
        │
        ▼
TranslatorPort
```

和划词翻译形成两条入口：

```text
划词翻译
Selection
   ↓
TranslateRequest

输入翻译
Input Text
   ↓
TranslateRequest
```

两者从 `TranslateRequest` 开始共享后续链路。

建议事件：

```rust
AppEvent::InputTranslationRequested {
    text,
}
```

空字符串应直接拒绝，不发送 HTTP 请求。

当前 Core 已提供 Selection 和 Input 两种入口。Input 路径绕过 `SelectionPort`，复用
`TranslationTaskId`、当前 Settings 和已有异步 `Translate` 命令链；空输入会使旧任务失效。

### M2.5 实现输入翻译 UI

状态：✅ 已完成

主窗口暂时只需要满足真实功能验证，不进行最终视觉精修。

目标界面：

```text
┌──────────────────────────────────────┐
│ English                 → 中文       │
│                                      │
│ Hello, how are you?                  │
│                                      │
│              Translate               │
├──────────────────────────────────────┤
│ 你好，你好吗？                       │
└──────────────────────────────────────┘
```

至少包含：

- 输入框
- 源语言 / Auto
- 目标语言
- Translate 按钮
- Loading 状态
- Translation Result
- Error 状态

要求：

- Translating 时 UI 不冻结
- 输入框保持响应
- Loading 状态即时出现
- Provider Error 能正确显示

此阶段不投入大量时间制作动画和复杂视觉效果。

当前主窗口已连接 `InputTranslationRequested`，输入编辑状态保留在 Slint，Core 状态单向
映射目标语言、Loading、结果与错误。翻译期间仍允许编辑和重新提交；Selection Demo
入口继续保留。

### M2.6 HTTP Infrastructure

状态：✅ 已完成

`lexift-translate` 通过共享的 reqwest Client 提供统一网络基础设施。Production Registry
在启动装配时创建一次 Client，并配置 5 秒连接超时、20 秒总超时、统一 User-Agent、
HTTPS、JSON 和系统代理发现。自动重试被显式关闭；Mock Registry 不创建网络 Client。
reqwest 错误在 Translate Adapter 边界转换为 `lexift_core::Error`。

### M2.7 第一个真实 Translator Provider

状态：✅ 已完成

第一阶段只实现 **一个真实 Provider**。

建议：

```text
DeepL
```

结构：

```text
lexift-translate/
├── providers/
│   ├── mod.rs
│   └── deepl.rs
│
├── client.rs
├── registry.rs
└── mock.rs
```

实现：

```rust
DeepLApiTranslator
```

并实现 Core：

```rust
TranslatorPort
```

要求：

- 使用长期复用的 HTTP Client
- 不允许每个请求重新创建 Client
- HTTP 逻辑全部留在 `lexift-translate`
- Provider-specific JSON 不进入 Core
- Provider-specific Error 转换成统一错误
- 设置合理的网络 Timeout

当前 Production Composition Root 根据 `LEXIFT_DEEPL_AUTH_KEY` 选择官方 DeepL API
Adapter；`:fx` Key 使用 Free Endpoint，其余 Key 使用 Pro Endpoint。Provider 复用 M2.6
共享 Client 配置，语言映射、请求/响应 Schema 和状态码错误映射均留在 Adapter 内。缺少
或空白 Key 时使用 `UnconfiguredTranslator`，应用仍可启动；`m1-demo` 始终使用 Mock。

### M2.8 Real Translation Hardening

状态：✅ 已完成

官方 Adapter 已重命名为 `deepl_api`，并明确与未来 DeepL Web、DLX Adapter 分离。
中文映射区分 `ZH-HANS` 与 `ZH-HANT`。localhost HTTP 测试覆盖请求协议、成功响应、
403、429、456、5xx、Malformed JSON、空响应、超时、连接失败和真实 HTTP 并发下的
stale result；默认测试不访问公网。Production UI 隐藏 Selection Demo，`m1-demo` 继续
使用 Mock Selection 与 Mock Translator。GitHub CI 执行格式、Clippy 和 Workspace 测试。
M2 阶段继续通过 `LEXIFT_DEEPL_AUTH_KEY` 临时注入 Credential；系统级 Secret Storage
留到 Desktop Integration 阶段。

---

## 4. M2 测试与验收

M2 完成必须满足：

```text
启动 Lexift

输入：
Hello world

目标：
zh-CN

点击 Translate

↓

立即进入 Translating

↓

UI 保持响应

↓

发起真实 HTTP 请求

↓

收到翻译结果

↓

进入 Success

↓

显示真实翻译
```

必须覆盖以下情况：

```text
正常翻译
空文本
错误 API Key
网络异常
Provider 超时
Provider 返回异常
快速连续发起多个请求
旧请求晚于新请求返回
```

至少应保证 Core reducer 和关键异步调度逻辑有自动化测试。

---

## 5. M3 — Windows Global Selection

M2 通过之后开始 M3。

M3 是 Lexift 第一次真正实现：

> **Translate Everywhere**

目标：

```text
任意 Windows 应用
      ↓
选中文字
      ↓
全局快捷键
      ↓
Lexift 获取 Selection
      ↓
翻译
      ↓
显示 Popup
```

### M3.1 Windows Global Hotkey

实现：

```text
HotkeyPort
```

Windows Adapter：

```text
RegisterHotKey / Windows native API
```

初始默认快捷键可以采用：

```text
Alt + X
```

但必须为未来用户配置留下结构。

事件链：

```text
Windows Hotkey
     ↓
Platform
     ↓
AppEvent::SelectionTranslationRequested
```

### M3.2 Windows Selection

Selection 采用多级策略：

```text
Strategy 1
Windows UI Automation

        ↓ fail

Strategy 2
Clipboard fallback
```

推荐设计：

```text
WindowsSelectionProvider
      │
      ├── UiAutomationSelection
      │
      └── ClipboardSelectionFallback
```

不能让 Core 知道 fallback 的存在。

Core 仍然只调用：

```text
SelectionPort
```

### M3.3 Selection 验证范围

至少验证：

```text
Chrome
Edge
VS Code
Word / Office
记事本
PDF Reader
Telegram / Discord 类应用
```

记录每种应用：

```text
UI Automation 成功
Clipboard fallback
不支持
```

不要因为某个应用不支持就向 Core 写特殊判断。

特殊兼容逻辑只能存在于 Platform Adapter。

---

## 6. M3 验收标准

必须实现：

```text
Chrome 中选中：
Hello world

      ↓

Alt + X

      ↓

Lexift 获取文字

      ↓

真实 Translator

      ↓

弹出：

Hello world
你好，世界
```

用户不需要：

```text
复制
切换窗口
粘贴
点击 Translate
```

这时 Lexift 才第一次真正实现产品定位：

> **Translate Everywhere.**

---

## 7. M4 — Desktop Integration

M3 后开始把技术 Demo 变成可以长期运行的桌面软件。

重点包括：

```text
Popup Position
System Tray
Settings
Hotkey Config
Provider Config
Target Language
Credential Store
Startup / Shutdown
Error UX
```

### Popup

实现：

```text
Selection.anchor
或者
Current cursor position
        ↓
Popup positioning
```

需要处理：

```text
屏幕边缘
多显示器
DPI Scaling
Popup 不遮挡原文
```

Popup 逐渐成为 Lexift 的核心 UI。

### Tray

Lexift 应采用：

```text
后台常驻
```

而不是：

```text
每翻译一次启动一次进程
```

Tray 最小功能：

```text
Lexift
├── Input Translation
├── Settings
└── Quit
```

后续再增加 Clipboard Mode 等能力。

### Settings

第一版至少：

```text
Target Language
Global Hotkey
Translation Provider
Theme
```

真正连接：

```text
Slint Settings
    ↓
Core
    ↓
SettingsStore
    ↓
lexift-config
```

### Credential Store

这一阶段再正式实现：

```text
CredentialStore
```

平台实现：

```text
Windows
    Windows Credential Manager

macOS
    Keychain

Linux
    Secret Service
```

普通配置文件只保存：

```text
credential_id
```

不保存 Secret。

---

## 8. V0.1 验收目标

完成 M2、M3、M4 后形成：

# Lexift V0.1

V0.1 不追求 Pot 功能完整度。

只要求把最核心体验做到稳定：

```text
系统启动
 ↓
Lexift 后台常驻

用户在任意应用选中文字
 ↓
全局快捷键
 ↓
获取 Selection
 ↓
Popup 快速出现
 ↓
真实翻译
 ↓
展示结果
```

同时具备：

```text
输入翻译
目标语言配置
Provider 配置
全局快捷键配置
系统托盘
基本错误处理
安全 Credential 存储
```

---

## 9. 暂不实现

V0.1 前明确不做：

```text
OCR
截图翻译
TTS
生词本
插件系统
AI Explain
AI Rewrite
多 Provider 并行
macOS
Linux X11
Linux Wayland
复杂动画
完整历史记录
```

这些不是删除能力，而是延后。

原则：

> 不让横向功能扩张破坏核心主链路开发。

---

## 10. 当前立即执行顺序

当前从 M1 进入 M2，实际开发顺序固定为：

```text
① Mock / Production Adapter 分离 ✅
        ↓
② TranslationTaskId ✅
        ↓
③ Settings.target_language 接入 Core ✅
        ↓
④ Input Translation Event / Use Case ✅
        ↓
⑤ Slint 输入翻译 UI ✅
        ↓
⑥ HTTP Client 基础设施 ✅
        ↓
⑦ DeepL Translator + Environment Credential ✅
        ↓
⑧ Real Translation Hardening ✅
        ↓
M2 验收 ✅
```

M2 完成后：

```text
Global Hotkey
    ↓
Windows Selection
    ↓
Clipboard fallback
    ↓
真实划词翻译
```

进入 M3。

---

## 11. 当前阶段最高优先级

Lexift 当前所有开发决策都应该围绕下面这一条判断：

> **这项工作是否让“选中文字 → 快捷键 → 快速得到翻译”更接近真实可用？**

如果答案是否定的，例如：

```text
OCR
插件系统
复杂主题系统
多个 Provider
TTS
AI Chat
复杂动画
```

当前阶段原则上不优先实现。

当前主线只有：

> **M2 真实翻译 → M3 Windows 全局划词 → M4 桌面集成 → V0.1。**
