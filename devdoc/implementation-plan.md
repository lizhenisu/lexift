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
窗口内选择属于 UI draft；Cancel 或关闭窗口会丢弃 draft，每次打开都从 Core 的 committed
Settings 重新同步。Save 通过 `SettingsSaveRequested → PersistSettings` 进入 Controller，磁盘写入
由 Tokio `spawn_blocking` 执行。只有 Store 保存成功后，`SettingsSaved` 才更新
`AppState.settings` 并关闭窗口；失败会保留旧设置，在设置窗口独立显示错误，不改变翻译状态。

Production 使用 `FileSettingsStore` 从操作系统用户配置目录加载设置。在 Windows 上路径为：

```text
%APPDATA%\Lexift\config.toml
```

当前磁盘 schema 为：

```toml
schema_version = 1

[settings]
target_language = "zh-CN"
```

配置文件不存在时使用默认值且不主动创建；解析、权限或未来版本错误会记录 warn 并继续启动，
也不会自动覆盖原文件。用户主动保存时创建父目录，并通过原子替换提交完整 TOML。Core 不依赖
serde/TOML，配置文件不保存 Secret；DeepL credential 继续由 `LEXIFT_DEEPL_AUTH_KEY` 提供。

Main Window 与 Windows Tray 的 Settings 入口共享同一 Core event。`m1-demo` 和自动测试使用
内存或临时目录 Store，不访问真实用户配置目录。

下一步：**M4.4 — Secure Credential Store**。

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
