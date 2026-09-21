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
 ✅ 核心验收通过
        │
        ▼
M4  Desktop Integration
 ← 当前阶段
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

当前实现由 Composition Root 通过 `SettingsStore` 加载并注入 `AppState`，Core 创建
`TranslateRequest` 时读取 committed `Settings.target_language`，不再硬编码目标语言。

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

M2.7 时 Production Composition Root 根据 `LEXIFT_DEEPL_AUTH_KEY` 选择官方 DeepL API
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
M2 阶段通过 `LEXIFT_DEEPL_AUTH_KEY` 临时注入 Credential；M4.4 已由系统级 Secret Storage
替代该生产链路。

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

状态：✅ 已完成

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

Windows Production 通过 `RegisterHotKey(None, ...)` 注册 `Alt+X`，使用 `MOD_ALT |
MOD_NOREPEAT`，并在独立的阻塞式 `GetMessageW` 线程接收 `WM_HOTKEY`。注册过程通过握手
同步报告冲突；失败只记录 warning，不阻止输入翻译或应用启动。`WindowsHotkeyPort` 在
Drop 时向 listener 投递 `WM_QUIT`、注销快捷键并 join 线程。当前 Windows capability 为
`hotkey = Some`；`m1-demo` 和非 Windows 均不装配该 Adapter。

### M3.2 Windows UI Automation Selection

状态：✅ 已完成

Windows Production 已装配 `WindowsSelectionPort`。划词请求先在现有
`spawn_blocking` worker 上捕获 Selection，成功或失败后才显示 Popup，避免 Popup 抢走
前台应用焦点。捕获失败通过独立的 `SelectionCaptureFailed` 事件进入 Error 状态，不影响
输入翻译的错误行为。

每次捕获都在调用线程初始化 MTA COM apartment，并创建 `CUIAutomation` client。选择传统
client 是为了保持 Chromium 等 proxy provider 的兼容性。Adapter
从 focused element 开始，在 Control View 中最多向上查找 8 层 `TextPattern`，读取并按原顺序
合并有效 selection ranges。单个节点的 pattern/provider 路径不可用时继续尝试父节点，空选择
返回 `None`；COM 初始化、client 创建、foreground/focus 获取异常转换为稳定的 Core Error。
Windows capability 现在为：

```text
hotkey    = Some(WindowsHotkeyPort)
selection = Some(WindowsSelectionPort)
```

本阶段只实现 UI Automation，`Selection.anchor` 保持 `None`；未加入剪贴板模拟、定位矩形或
应用特判。

### M3.3 Clipboard Selection Fallback

状态：✅ 已完成

在 UI Automation 返回无有效 Selection 时，使用 Clipboard fallback：

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

Windows Selection 现在先尝试 UI Automation；未得到选择或 UIA 调用失败时，在独立 STA
线程中事务式执行 `Ctrl+C` fallback。事务在覆盖前通过 `OleGetClipboard` 访问原内容，
将各格式数据保存到独立 Shell `IDataObject`，读取 Unicode 文本后使用
`OleSetClipboard` 和 `OleFlushClipboard` 恢复内容；
若复制后用户或其他应用再次更新剪贴板，则通过 sequence number
检测并保留更新内容。快捷键释放、剪贴板打开和复制等待均有短超时，未发生 sequence 变化时
不会读取旧剪贴板。UIA 与 Clipboard 的组合策略仍封装在 Platform Adapter 内，Core Port
保持不变。受 Windows UIPI 限制，普通权限 Lexift 暂不能向管理员权限进程可靠发送复制输入；
本阶段不自动提权。

#### M3.3.1 Clipboard Restore Correctness

状态：✅ 已完成

直接恢复 `OleGetClipboard` 返回对象曾在人工测试中出现
`CLIPBRD_E_CANT_CLOSE (0x800401D4)`。现改为复制前通过
`EnumFormatEtc/GetData` 将各格式的 OLE storage medium 存入独立 Shell `IDataObject`，
由 `SetData` 接管所有权；保存失败则不发送 Ctrl+C。恢复继续使用 OLE API。
新增不操作全局剪贴板的测试，确认释放源对象后快照仍可读取。

2026-09-14 用户本机运行 `interactive_selection_capture_restores_text_clipboard`
通过（1 passed）：剪贴板 sequence 发生变化、读取到非空 Unicode 选区、fallback 结束后
`sentinel preserved=true`。这确认了文本剪贴板的取词和恢复链路。
同日用户本机运行 `interactive_selection_capture_restores_empty_clipboard` 通过
（1 passed）：复制后读取到非空 Unicode 选区，fallback 结束后 `format count=0`，
确认恢复为没有任何格式的空剪贴板。测试初始化用的 OLE apartment 在倒计时前释放，
避免休眠的 STA 窗口阻塞目标应用的剪贴板消息。
同日用户在资源管理器复制文件、执行一次 Lexift 划词翻译后粘贴成功；恢复后的文件名、
大小一致且可以正常打开。

同日用户本机运行 `interactive_newer_clipboard_content_wins` 通过（1 passed）：Lexift 读取
选区后，测试写入新的剪贴板内容，结束时 `newer content preserved=true`。这确认 sequence
guard 会在剪贴板再次变化时放弃旧快照，用户的新内容不会被覆盖。

同日用户运行图片和文件专用的强制 Clipboard fallback 测试。文件测试
`interactive_selection_capture_restores_file_clipboard` 通过（1 passed），恢复后报告
`original formats preserved=true`；图片测试同样通过。两类内容在测试后均可正常粘贴，
图片内容和尺寸一致，文件名、大小和内容一致。

剪贴板恢复不再枚举、复制和手工释放不同格式的 native handle。原内容以 OLE
`IDataObject` 快照保存，恢复成功并完成 `OleFlushClipboard` 后才消费快照；失败时 RAII
仍会在 sequence guard 允许的前提下重试。原剪贴板为空时通过 `OleSetClipboard(None)`
恢复空状态。`SendInput` 部分失败会补发 `C Up` 和 `Ctrl Up`，避免残留按键状态。

### M3.4 Windows Selection Compatibility & End-to-End Hardening

状态：✅ 核心验收通过；部分兼容性覆盖待补，可进入 M4

Selection capture 现在记录 `uia_latency_ms`、`clipboard_latency_ms` 和
`total_capture_ms`，且事件日志只记录事件类型，不再通过 `Debug` 输出事件载荷。无选区使用
独立的 `SelectionCaptureEmpty` 事件回到 `NoSelection` 状态，不弹错误 Popup。Controller
对 Selection capture 实施单飞门控，捕获尚未结束时重复 `Alt+X` 不会启动第二个 UIA 或
Clipboard transaction。

兼容记录和复测步骤见 `devdoc/windows-selection-compatibility.md`。当前环境检测到 Chrome
153.0.8010.37、Edge 153.0.4234.32、VS Code 1.137.0、Notepad 和 Word
16.0.20326.20144；Telegram、Discord 和独立 PDF Reader 未发现。真实 DeepL Provider
测试已通过。用户已验证上述已安装应用及浏览器 PDF 的划词、焦点保护与错误恢复；
A → B 重叠翻译和长按 Alt 空选区复测通过。剪贴板恢复沿用 M3.3.1 与本轮测试记录。
未安装应用及逐应用策略、延迟仍待补，不阻塞 M4，也不视为全矩阵验收完成。

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

### M4.1 Popup Positioning

状态：✅ 已完成，桌面验收通过

2026-09-15 用户完成单屏位置、多显示器及不同缩放测试，确认 Popup 始终位于鼠标附近，
各位置均符合预期。测试另发现 Popup 偶尔被其他应用遮挡；现通过 Slint
`always-on-top: true` 将翻译 Popup 设为置顶窗口。同日用户复测确认 Popup 不再被遮挡，
原应用输入焦点正常。

后续复测：无选区时隐藏旧 Popup 已通过；点击关闭后持续按 Alt、重复按 X 出现翻译与
无选区交替的问题。现为 Popup 增加 Windows `WS_EX_NOACTIVATE`，关闭按钮通过
`PopupHidden` 同步 Core 状态，并在重新显示前应用非激活策略。用户复测确认关闭后重开、
持续按 Alt 重复按 X 的交替消失问题已解决。

Windows Selection capture 开始时通过 `GetCursorPos` 保存物理屏幕 Anchor；Anchor 获取失败
不会影响文字捕获。App 通过可选 `ScreenPort` 使用 `MonitorFromPoint` 和
`GetMonitorInfoW.rcWork` 取得目标显示器工作区，再把定位上下文交给 UI。非 Windows 或屏幕
能力失败时仍按窗口系统默认位置显示。

Slint 在 `show()` 前读取 Popup 的物理尺寸，优先放在 Anchor 右下方；空间不足时分别翻转到
左侧或上方，最后按工作区边距约束。算法保留虚拟屏幕负坐标，并以纯测试覆盖四个方向、
窗口过大和左侧副屏。`SelectionCaptureEmpty` 继续进入 `NoSelection`，不会显示 Popup。

定位链路：

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

下一步：**M4.2 — System Tray & Background Lifecycle**。

### M4.2 System Tray & Background Lifecycle

状态：✅ 实现与人工验收通过

2026-09-15 用户复测：关闭主窗口后托盘保留，左键恢复主窗口且输入内容保留；
Explorer 重启后图标恢复，Alt+X 翻译正常。修复悬停提示缺失与右键菜单闪烁后，
用户再次确认：悬停提示正常、右键菜单稳定显示且点击外部正常关闭、
Open Lexift 与 Quit 正常，Quit 后进程消失。M4.2 人工验收通过。

修复包含 Version 4 的 NIF_SHOWTIP，仅处理
NIN_SELECT/NIN_KEYSELECT/WM_CONTEXTMENU，增加菜单重入保护，
并在执行菜单动作前完成 NIM_SETFOCUS。自动测试覆盖旧通知过滤和重入释放。

Windows Production 现在装配可选 `WindowsTrayPort`。Adapter 在独立 Win32 message thread
创建隐藏窗口，通过 `Shell_NotifyIconW` 注册通知区域图标并阻塞在 `GetMessageW`，不使用轮询。
左键激活与右键菜单的 `Open Lexift` 映射为 `MainWindowRequested → ShowMainWindow`；`Quit`
复用 `ExitRequested → Exit → quit_event_loop`。右键菜单使用 `SetForegroundWindow`、
`TrackPopupMenu`、`WM_NULL` 和 `NIM_SETFOCUS` 维护原生菜单焦点。

主窗口改用 `run_event_loop_until_quit`。托盘注册成功后，关闭主窗口只隐藏同一个 Slint
实例，Hotkey 和后台翻译继续运行；注册失败或平台无 Tray capability 时，关闭主窗口仍正常
退出，避免产生无法找回的后台进程。Tray Open 会恢复隐藏或最小化的主窗口，不影响 Popup。

Tray thread 通过注册握手报告 `NIM_ADD` 和 `NIM_SETVERSION` 的结果。Drop 时先删除图标，
再销毁隐藏窗口、注销 window class 并 join thread。收到 `TaskbarCreated` 后重新添加图标，
用于恢复 Explorer 重启清除的通知区域状态。当前使用系统 `IDI_APPLICATION`，正式图标留到
M4.6 Packaging。

原生注册与关闭冒烟测试、菜单 command 映射、Core reducer、Controller View 调用、托盘失败
降级和关闭策略均有自动化覆盖。

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

下一步：**M4.3 — Settings & Persistence**。

### M4.3 Settings & Persistence

状态：✅ 实现与桌面人工验收通过

2026-09-15 用户完成桌面人工验收，确认 Settings 与 Target Language 持久化链路可用。

Settings Window 第一版只开放 Target Language，提供 12 个 Lexift canonical language code。
M4.5.1 起，语言、快捷键与 Provider 改为操作完成后单项保存；页面级 Cancel/Save 已移除。
Credential 仍通过字段内 Save/Edit Save/Cancel 显式提交，避免逐字符写入系统安全存储。
磁盘写入由 Tokio `spawn_blocking` 执行；持久化或运行时应用失败会恢复对应字段的旧值并就地
显示错误，不改变其他设置或翻译状态。

Production 使用 `FileSettingsStore` 从操作系统用户配置目录加载设置。在 Windows 上路径为：

```text
%APPDATA%\Lexift\config.toml
```

当前磁盘 schema 为：

```toml
schema_version = 4

[settings]
target_language = "zh-CN"
provider = "deepl"
launch_at_login = false

[settings.hotkey]
control = false
alt = true
shift = false
meta = false
key = "X"

[credentials]
deepl = "deepl-primary"
```

配置文件不存在时使用默认值且不主动创建；解析、权限或未来版本错误会记录 warn 并继续启动，
也不会自动覆盖原文件。用户主动保存时创建父目录，并通过原子替换提交完整 TOML。Core 不依赖
serde/TOML，配置文件不保存 Secret；`credentials.deepl` 只保存系统 Credential Store 中
`Lexift/<credential_id>` 的引用。

Main Window 与 Windows Tray 的 Settings 入口共享同一 Core event。`m1-demo` 和自动测试使用
内存或临时目录 Store，不访问真实用户配置目录。

#### Settings 候选列表覆盖层

2026-09-21 将 Target Language 与 Provider 候选列表从独立的 `LanguageMenuWindow` 改为
`SettingsWindow` 内部的共享覆盖组件。列表与 Settings 使用同一个 HWND，拖动、快速晃动和调整
窗口大小时天然同步，也不会改变 Settings 的激活标题栏或窗口阴影。

候选列表统一从对应选择框下方展开，宽度与选择框一致。实际高度取候选内容高度和窗口底部
剩余可见高度的较小值；空间不足时保持 40px 行高，通过列表内部 ScrollView 滚动，不再切换
到窗口上方或越过原生窗口边界。打开语言列表时会将当前选中项滚动到可见区域中部。

Settings 页面本身使用独立 ScrollView。候选列表的位置直接绑定选择框的 `absolute-position`，
页面滚动时保持锚定；选择框离开可见区域时关闭。覆盖层顺序固定为页面内容、外部点击关闭层、
候选列表和 Toast，避免列表吞掉 Toast、Credential 操作或其他字段事件。

选择项仍先更新 UI 草稿并发送原有单项自动保存事件，再关闭列表。点击列表外区域、Escape、
隐藏或关闭 Settings 时也会关闭。语言和 Provider 共用同一组件与同一打开状态，不允许同时
显示多个列表。

独立候选窗口所需的原生 owner、`WindowContextMonitor`、鼠标/前台 Hook、120ms Opening 状态、
75ms 窗口监测和屏幕坐标计算已经删除。TranslationPopup 仍保留
`WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW`，其焦点策略不受此次重构影响。

#### Settings 响应式分类面板

2026-09-21 将 Settings 从单列长表单重构为固定侧栏、固定页面标题和分类内容区。默认窗口尺寸为
820×680，最小尺寸为 520×520；窗口宽度达到 720px 时侧栏显示 Lexift 标识及分类名称，低于
该宽度时收缩为 64px 图标栏并通过 tooltip 提示名称。

当前分类为 General 和 Translation。General 包含 Language & region、Application 卡片，承载
目标语言、快捷键和开机启动；Translation 包含 Provider、Credentials 卡片，承载翻译 Provider
和 DeepL API Key。宽度达到 800px 时卡片双列排列，窄窗口自动切换单列，只有当前分类的内容区
滚动，侧栏、标题和 Toast 保持固定。

卡片、设置行、侧栏项和 tooltip 使用统一 Design Tokens。候选列表继续使用同一 Settings HWND
内的覆盖层。切换分类会关闭候选列表与快捷键捕获；离开 Translation 时通过现有 Credential
清理回调隐藏明文、取消编辑并清空未保存 Secret。关闭或重新打开 Settings 时始终回到 General，
配置 schema、单项自动保存、错误回退和 Credential Store 边界保持不变。

当前里程碑：**M4.4 — Secure Credential Store**。

### M4.4 Secure Credential Store

状态：✅ 实现完成并通过桌面人工验收

Core 已定义线程安全的 `CredentialStore` Port，以及 `Missing`、`PermissionDenied`、
`PlatformFailure`、`InvalidFormat` 四类稳定错误。Secret 使用自定义 `CredentialSecret`，其
`Debug` 始终输出 `[REDACTED]`；AppState 只保存 `credential_configured`、忙碌/错误状态和
credential reference，不保存 Key。

Windows Adapter 使用 `CredWriteW`、`CredReadW`、`CredDeleteW` 管理 Generic Credential，统一
target 为 `Lexift/<credential_id>`，当前 DeepL 使用 `Lexift/deepl-primary`。`CredReadW` 的
native buffer 在复制完成后立即通过 `CredFree` 释放。macOS Keychain 和 Linux Secret Service
留给对应平台 Adapter 实现。

配置 schema 在 M4.5 升级到 v3，并在 M4.6 升级到 v4；v1-v3 文件加载时自动补充 Runtime
Configuration 与 `launch_at_login = false` 默认值，credentials section 仍只保存引用。普通配置只保存：

```toml
schema_version = 4

[settings]
target_language = "zh-CN"
provider = "deepl"
launch_at_login = false

[settings.hotkey]
control = false
alt = true
shift = false
meta = false
key = "X"

[credentials]
deepl = "deepl-primary"
```

Settings Window 使用统一凭证字段。未配置时显示密码输入框和 Save key；已配置时只显示固定
全掩码 `••••••••••••••••`，不泄露首尾字符。字段 hover 时提供 Slint `Path` 绘制的查看、复制、
编辑、删除操作，避免字体图标和平台字体差异。

#### M4.4.1 可查看、复制和编辑的 Credential 字段

状态：✅ 实现完成并通过桌面人工验收

四个操作图标采用 Google Material Outlined 的 24×24 矢量几何路径，不依赖 Material Symbols
字体或 Unicode 字符。界面按 Design Tokens 统一为 32×32 操作区域、20×20 图标和 16px 圆形
状态层；默认颜色使用 secondary foreground，hover 使用 8% state layer，pressed 使用 12%
state layer。查看状态使用 `visibility` / `visibility_off` 路径切换，Reveal 和 Edit 中的图标均
反映当前明文可见状态。

Configured idle 状态始终保留四个图标的布局实例，仅通过透明度显示或隐藏。Credential 字段只
使用一个覆盖全区域的 `TouchArea` 处理 hover、pressed 和点击，并按鼠标横坐标把点击路由到四个
固定操作区域；此状态下图标组件只负责绘制，不创建独立命中层。Reveal/Edit 状态下需要独立交互
的眼睛按钮才启用自己的点击区域。

这是该组件的回归约束：不得根据同一字段的 hover 状态动态创建或销毁操作栏，也不得在字段
`TouchArea` 上方叠加会截获指针的子 `TouchArea`。前一种实现会让新出现的子组件夺走字段 hover，
造成操作栏反复出现和消失；后一种实现会让鼠标按下与释放落在不同组件上，导致查看、复制、编辑、
删除回调无法完成。`credential-request-pending` 继续作为异步操作门闩，generation 继续隔离过期
读取结果和计时回调。

查看和编辑由 `CredentialAccessPurpose` 驱动，在 blocking worker 中临时读取 CredentialStore。
Reveal/Edit 的 Secret 通过一次性 ViewPort 调用送入 Settings，不进入 AppState；Reveal 在字段失焦、
Settings 隐藏/关闭或 30 秒后清除。Edit 默认保持密码遮罩，Save 复用安全保存事务，Cancel 清除
draft。Copy 由 AppController 直接组合 CredentialStore 与 `ClipboardPort`，UI 只收到 `Copied`
反馈，不接收完整 Secret。Windows Clipboard Adapter 使用 `CF_UNICODETEXT`、可移动 Global Memory
和有限 OpenClipboard 重试；`SetClipboardData` 成功后正确转移内存所有权。

每次临时读取分配 generation。关闭窗口、隐藏明文、取消编辑、保存/删除成功及应用退出都会递增
generation 并清空 transient secret、draft、明文标志和反馈，过期 worker 结果与超时回调无法覆盖
新会话。复制是用户明确触发的导出，内容保留在系统剪贴板，不自动覆盖。

首次输入值只作为 UI draft 进入 `CredentialSaveRequested`，保存成功后立即清空。
保存或删除在 Tokio blocking worker 中执行，并把 Credential Manager 与 config reference 作为
一个逻辑提交：配置保存失败时恢复原凭证，避免部分提交。

生产翻译不再读取 `LEXIFT_DEEPL_AUTH_KEY`。Composition Root 的 credential-backed translator
在每次翻译前按当前 reference 临时读取 Secret，用共享 HTTP Client 创建短生命周期的官方
DeepL Adapter，请求结束后释放；DeepL Adapter 本身不知道 CredentialStore。缺少凭证只让翻译
不可用，不阻止 Tray、Hotkey 或应用启动。保存或删除凭证后运行时 reference 立即更新。

桌面人工验收需确认保存后 Windows Credential Manager 出现 `Lexift/deepl-primary`、重启后状态
仍为 Configured 且翻译可用；鼠标在字段和四个图标间移动时操作栏稳定且不闪烁；查看可显示并
再次隐藏 Key，复制后出现 `Copied` 且能粘贴完整 Key，编辑的 Save/Cancel 正常，Remove 后条目
消失；明文在失焦、Settings 关闭或 30 秒后清除。最后检查 config.toml、AppState 和日志均不含
Key。

完成验证包括默认与 `m1-demo` Workspace 测试、全 Workspace Clippy、Windows Credential Manager
临时凭证写入/读取/删除集成测试，以及配置持久化与 Secret 脱敏测试。生产启动和翻译链路不读取
环境变量；`LEXIFT_DEEPL_AUTH_KEY` 仅保留给显式忽略、需要人工运行的 DeepL 网络适配器测试。

当前里程碑：**M4.6 — Windows V0.1 Desktop Release**。

### M4.5 Runtime Configuration

状态：✅ 实现完成并通过桌面人工验收

Core 新增独立的 `RuntimeConfig`，包含当前已加载的 `HotkeyConfig` 和 `ProviderConfig`。Settings
仍表示持久化的用户偏好；`AppState.runtime_config` 表示当前运行实例已经成功应用的配置，两者
只在完整事务成功后一起提交。Runtime 错误写入独立的 `runtime_config_error_message`，不会覆盖
翻译错误。

设置保存链路调整为：

```text
SettingsChangeRequested
  ↓
PersistSettings
  ↓
RuntimeConfigChanged
  ↓
RuntimeManager.apply
  ↓
RuntimeConfigUpdated / RuntimeConfigUpdateFailed
```

运行时应用失败时，Controller 会把磁盘设置恢复为此前 committed Settings，Core 保留旧的
Settings 与 RuntimeConfig，Settings Window 保持打开并显示运行时错误。UI 不直接访问 Hotkey
或 Translator Adapter。

`HotkeyRuntimeManager` 保存当前配置和事件 handler。Windows Adapter 的动态切换先在新的监听线程
注册新组合，注册成功后才替换并关闭旧监听线程；系统冲突或注册失败时旧快捷键继续工作。支持
Ctrl、Alt、Shift、Win 与 A-Z、0-9、F1-F12，至少需要一个修饰键。Settings 使用 FocusScope 捕获
实际键盘事件，Escape 取消本次捕获。

`TranslatorRuntime` 保存当前 Provider 状态，并在 Provider 修改时检查对应 Credential。当前 Provider
只有 DeepL，因此 UI 的单选下拉列表只展示 DeepL，不展示尚未实现的选项。翻译请求仍按当前 credential reference
临时读取 Secret，再由共享 `DeepLTranslatorFactory` 创建短生命周期 Adapter；Provider Adapter
不持有 CredentialStore。

配置 schema 升级到 v3；v1/v2 自动迁移为 `Alt + X` 与 DeepL。自动测试覆盖 Hotkey 初始注册、
动态切换、失败保留旧组合、Provider credential validation、Settings 持久化后再应用 Runtime、
运行时错误隔离及旧 schema 迁移。Windows 原生集成测试也覆盖了真实 `RegisterHotKey` 注册、
原子替换、旧组合释放、新组合冲突和 Drop 清理。

桌面人工验收需确认：捕获新快捷键后无需重启即可触发划词翻译；旧快捷键立即失效；选择一个
被系统占用的组合时显示独立错误且旧快捷键仍可用；重启后新组合保持；Selection、Tray、Popup、
Settings、Credential 和 DeepL 翻译链路无回归。

下一步：**M4.6 — Desktop Release**。

### M4.5.1 Settings 单项自动保存与 Provider 交互

状态：✅ 实现完成并通过桌面人工验收

Settings 以 `SettingsChange` 表达 Target Language、Hotkey 和 Provider 的单项修改。Core 串行执行
保存事务，忙时保留其他字段请求，同一字段只保留最新值。成功后窗口保持打开；失败时只恢复对应
字段并显示字段级错误。

Runtime 按修改字段应用能力：Target Language 不访问平台或 Credential；Hotkey 只替换全局快捷键；
Provider 才验证并切换 Translator。因此未配置 DeepL API Key 时，语言和快捷键仍可正常保存。
磁盘保存或运行时应用失败继续执行旧设置回滚，旧快捷键与 Provider 保持可用。

Target Language 在选择列表项后保存，Hotkey 在捕获有效组合后保存。Provider 使用与语言列表相同的
owned transient menu 和外部交互关闭机制；当前菜单只有 DeepL，选择当前值只关闭菜单。DeepL API
Key 继续使用字段内显式保存，不在输入或失焦时自动写入 Credential Store。

桌面人工验收需确认：无 DeepL Key 时可以修改语言与快捷键；快捷键修改立即生效；Provider 点击可
稳定展开；失败字段恢复旧值且错误显示在字段附近；关闭 Settings 不撤销已经成功保存的项目；
Credential 查看、复制、编辑、删除及显式保存无回归。

### M4.5.2 Settings 稳定渲染与独立 Toast 反馈

状态：✅ 实现完成并通过桌面人工验收

Settings 保存期间不再创建 `Saving…` 布局项，也不再用全局 busy 状态禁用无关字段。UI 使用
`desired_settings` 呈现用户的最新选择，并只在 Slint 属性实际变化时写入对应字段。因此保存 API Key
不会改变 Shortcut 按钮的视觉状态，修改语言、快捷键或 Provider 也不会触发整页重排或无关控件重绘。

Settings 修改仍由 Core 串行提交；快速修改同一字段时只保留最新期望值。旧事务失败时，如果该字段
已经有更新请求，则不会用旧 committed 值覆盖用户的新选择。Credential 写操作可以在普通 Settings
事务执行期间发起，由 Controller 等待当前事务结束后读取最新 committed Settings，再安全地更新
Credential reference；Secret 不进入 AppState、普通配置或日志。

保存和 Credential 操作结果通过 Settings 顶部悬浮 Toast 展示。每条 Toast 都是独立实例，拥有唯一
ID、关闭按钮和自己的自动关闭计时器；成功项约 2 秒消失，失败项约 5 秒消失。快速连续操作会同时
显示多条 Toast，并按垂直阶梯排列；单条手动关闭或计时结束只移除自身，其余 Toast 自动补位。
Toast 不参与页面布局，失败详情仍保留在对应字段附近。Settings 隐藏、关闭或重新打开时清空现有
Toast，旧计时器不能影响之后创建的实例。

桌面人工验收需确认：API Key 保存时 Shortcut 按钮不闪烁；修改 Shortcut、语言或 Provider 时页面
不移动、不闪烁；保存期间其他字段仍可操作；快速连续完成多项操作时多条 Toast 同时显示、分别按
时消失并可单独关闭；失败字段回退和就地错误正常。

### M4.6 Windows V0.1 Desktop Release

状态：🚧 实现完成，等待 Release 产物与安装/卸载人工验收

Windows Release 使用 GUI subsystem，嵌入 Lexift 多尺寸图标、应用清单和 PE 版本信息。系统托盘
复用同一嵌入图标。应用使用命名互斥量保证单实例；第二次启动通过隐藏的原生协调窗口通知已有实例
打开主窗口，不创建第二套 Runtime、Hotkey 或 Tray。发布脚本覆盖同名安装器后通过定向的 Windows
Shell 更新通知刷新该文件图标，避免本地反复打包时 Explorer 继续显示旧图标缓存。

Settings 新增 Launch at startup。配置 schema 升级到 v4，旧 v1-v3 自动补充 `false`。Windows
Adapter 只写入当前用户 Run 注册表项，命令固定为带引号的绝对路径与 `--background`；启动时会校验
并修复安装路径变化。后台模式创建完整运行时和托盘，但不主动显示主窗口。卸载模板始终删除启动项、
安装器产品键、卸载项和当前安装路径对应的托盘历史；不扫描或删除其他 Lexift 构建的托盘记录。

卸载确认页的“删除用户数据”默认不选中。未选中时保留 `%APPDATA%\Lexift` 配置、
`%LOCALAPPDATA%\Lexift` 日志和 Windows Credential Manager 中的 DeepL API Key，确保覆盖安装与升级
不丢失数据；选中时递归删除两个数据目录并删除 `Lexift/deepl-primary` 凭证。目录或凭证清理失败会
写入卸载详情并把卸载标记为存在错误，避免静默报告完全清理成功。

NSIS 的目录删除路径固定使用 Windows 反斜杠；隔离验证确认正斜杠路径不会删除目标目录。Release
验证会拒绝渲染后出现 `$APPDATA/Lexift` 或 `$LOCALAPPDATA/Lexift`。卸载成功回调会对启动项、卸载项
和产品键执行幂等的最终清理；安装器也只恢复确实包含 `uninstall.exe` 的旧安装路径，无效历史路径会
被忽略并删除。

Release 日志写入 `%LOCALAPPDATA%\Lexift\logs`，按日滚动并最多保留 5 个文件。发布工程包含
`packager.toml`、当前用户 NSIS 安装器、portable ZIP、SHA256 校验文件、可选 Authenticode 签名脚本
及 Tag 驱动的 GitHub Actions 工作流。正式 Tag 和 GitHub Release 仅在 Windows 桌面清单全部通过后创建。

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
    ✅
    ↓
Windows Selection
    ✅
    ↓
Clipboard fallback
    ✅
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
