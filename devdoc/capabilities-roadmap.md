# Lexift 功能能力与路线图

## 1. 文档目的

本文档用于记录 Lexift 在当前架构设计下可承载的功能能力、与 Pot 功能的对应关系，以及建议的分阶段实现路线。

Lexift 的目标不是简单复刻 Pot，而是以 **Rust + Slint + Native GUI** 为基础，在保持跨平台能力的同时，优先追求更低常驻资源、更快交互响应和更统一的桌面体验。

核心产品定位：

> **Translate Everywhere**

长期可进一步扩展为：

> **Understand Everything, Everywhere**

---

## 2. 当前架构可承载的核心能力

基于当前规划的模块划分：

- `lexift-app`
- `lexift-core`
- `lexift-ui`
- `lexift-platform`
- `lexift-translate`
- `lexift-config`
- `lexift-observability`

Lexift 可以逐步实现以下能力。

### 2.1 划词翻译

这是 Lexift 的第一核心能力。

基本链路：

```text
任意应用
  ↓
选中文字
  ↓
全局快捷键
  ↓
lexift-platform 获取 Selection
  ↓
lexift-core 创建翻译任务
  ↓
lexift-translate 调用 Provider
  ↓
lexift-ui 在合适位置显示结果
```

目标体验：

> 用户在任意支持的桌面应用中选中文字，按下快捷键，即可立即看到翻译结果。

---

### 2.2 输入翻译

通过全局快捷键主动呼出输入窗口，输入文本后翻译。

基本链路：

```text
UI
 ↓
Core
 ↓
Translate
 ↓
UI Result
```

该能力不依赖划词捕获，因此适合较早实现。

---

### 2.3 多翻译 Provider

`lexift-translate` 负责统一不同翻译服务。

未来可支持：

- Google Translate
- DeepL API（当前已实现）
- DeepL Web（未来 Experimental Adapter）
- DLX / DeepLX（未来 Optional Adapter）
- Bing
- OpenAI
- Gemini
- Ollama
- 其他在线或本地翻译服务

统一抽象后，Core 和 UI 不直接依赖具体 Provider。

DeepL API、DeepL Web 与 DLX 使用独立 `TranslatorPort` Adapter，各自维护认证、协议和
错误边界。Adapter 之间不隐式 fallback；未来如需 fallback，由上层显式路由策略决定。

可进一步支持多 Provider 并行翻译：

```text
TranslateRequest
      │
 ┌────┼─────┐
 ↓    ↓     ↓
A     B     C
 └────┼─────┘
      ↓
Result Aggregation
```

---

### 2.4 剪贴板监听翻译

通过平台层监听剪贴板变化，在满足规则时自动触发翻译。

需要处理：

- 文本类型判断
- 重复内容过滤
- Lexift 自身写入导致的循环触发
- 最小/最大长度限制
- 用户开关

推荐设计为事件驱动，而非高频轮询。

---

### 2.5 截图与截图翻译

截图翻译由多个独立能力组合完成：

```text
Screen Capture
      ↓
Image
      ↓
OCR
      ↓
Text
      ↓
Translator
      ↓
UI
```

建议避免设计一个耦合过重的 `ScreenshotTranslator`，而是保持截图、OCR、翻译之间可独立替换。

---

### 2.6 OCR

OCR 后续可统一抽象为 Provider。

可支持：

- Windows 系统 OCR
- Apple Vision
- Linux 本地 OCR
- Tesseract
- PaddleOCR
- 云 OCR 服务

第一阶段不要求单独拆出 `lexift-ocr` crate；当 OCR 能力复杂到足够独立时再拆分。

---

### 2.7 TTS / 朗读

后续可增加统一 Speech Provider，用于：

- 原文朗读
- 译文朗读
- 系统 TTS
- 在线 TTS

第一阶段不单独创建 speech crate。

---

### 2.8 生词本 / 收藏

翻译结果可以保存到：

- 本地生词本
- Anki
- 欧路词典
- 其他第三方服务

建议统一使用 `VocabularyEntry` / `CollectionProvider` 一类的领域模型和接口。

---

### 2.9 外部调用

Lexift 后续应支持被其他程序调用。

可能的入口包括：

```text
CLI
URL Scheme
Local IPC
```

例如：

```text
lexift translate "hello world"
```

长期可以通过 Named Pipe / Unix Socket 连接已经运行的 Lexift 实例，避免重复启动应用。

---

### 2.10 系统托盘与后台常驻

Lexift 应以常驻桌面工具为目标，而不是依赖长期打开主窗口。

典型托盘能力：

- 输入翻译
- 剪贴板监听开关
- 截图翻译
- 设置
- 退出

常驻状态下应尽量做到：

- CPU 接近零占用
- 无无意义高频轮询
- 不持续刷新 GPU
- 最小化后台线程和唤醒次数

---

### 2.11 国际化

UI 从早期就应避免硬编码中文或英文字符串。

至少优先支持：

- `zh-CN`
- `en-US`

后续可扩展：

- `zh-TW`
- `ja-JP`
- `ko-KR`
- 其他语言

---

### 2.12 插件系统

插件系统属于长期能力，不进入 V0.1。

第一阶段先稳定内部抽象，例如：

- `Translator`
- `OcrProvider`
- `SpeechProvider`
- `CollectionProvider`

等接口成熟后，再考虑：

```text
lexift-plugin-api
lexift-plugin-runtime
```

插件运行时优先考虑安全隔离和跨版本兼容，WASM 是候选方向之一，但当前尚未最终确定。

---

## 3. 与 Pot 的能力映射

当前架构可以覆盖 Pot 的大多数核心功能。

| Pot 能力 | Lexift | 主要模块 |
| --- | --- | --- |
| 划词翻译 | 可实现，核心功能 | `core + platform + translate + ui` |
| 输入翻译 | 可实现 | `core + translate + ui` |
| 全局快捷键 | 可实现 | `platform` |
| 剪贴板监听 | 可实现 | `platform + core` |
| 多翻译源 | 可实现 | `translate` |
| 多翻译源并行 | 可实现 | `translate + core` |
| 自动语言检测 | 可实现 | `core / translate` |
| 截图 | 可实现 | `platform` |
| OCR | 可实现 | `platform`，后续可拆 `ocr` |
| 截图翻译 | 可实现 | `platform + OCR + translate` |
| 系统托盘 | 可实现 | `platform + ui` |
| TTS | 可实现 | 后续 speech capability |
| 生词本 | 可实现 | 后续 collection/integration |
| 外部调用 | 可实现 | `app + core` |
| URL Scheme | 可实现 | `app + platform` |
| CLI 调用 | 可实现 | `app` 或后续独立 CLI |
| Windows | 目标支持 | `platform/windows` |
| macOS | 目标支持 | `platform/macos` |
| Linux X11 | 目标支持 | `platform/linux/x11` |
| Linux Wayland | 目标支持，但技术复杂度最高 | `platform/linux/wayland` |
| 国际化 | 可实现 | `ui + config` |
| 插件系统 | 可实现，建议 V2 以后 | 后续 plugin crates |
| API Key 安全存储 | 可实现 | `config + platform` |

---

## 4. Lexift 自身的扩展方向

Lexift 不应长期局限于 `translate()`。

未来可以围绕用户当前选中的内容提供统一动作入口：

```text
选中文字
   ↓
Translate
Explain
Define
Rewrite
Summarize
Pronounce
Copy
Ask AI
```

这种能力仍可以复用现有的：

- Selection
- Core event
- Provider/service abstraction
- Popup UI

因此无需推翻整体架构。

---

## 5. 建议实现路线

### V0.1：完成最小可用闭环

第一阶段严格控制范围，只实现最能体现 Lexift 价值的功能：

1. Windows 支持
2. 全局快捷键
3. 划词获取
4. 翻译 Popup
5. 输入翻译
6. 至少 1~2 个翻译 Provider
7. 基础设置
8. 系统托盘

核心验收标准：

> **选中文字 → 按下快捷键 → 快速看到翻译结果。**

V0.1 优先优化这一条链路的可靠性、延迟和 UI 体验。

---

### V0.2：覆盖 Pot 的主要日常能力

增加：

- Clipboard Translation
- 多 Provider
- Provider 并行
- OCR
- Screenshot
- Screenshot Translation
- macOS
- Linux X11

---

### V0.3：完善跨平台桌面能力

增加：

- Linux Wayland
- TTS
- 生词本
- Auto Start
- Proxy
- External Invocation
- URL Scheme
- CLI
- Local OCR

---

### V1.0：形成 Lexift 自己的产品能力

增加：

- AI Explain
- AI Rewrite
- Dictionary
- Context-aware Translation
- Translation History
- Search
- 可扩展插件系统

---

## 6. 当前优先级原则

开发顺序应遵循：

> **先把高频主路径做到极致，再增加功能数量。**

优先级高于“功能多”的指标包括：

- Popup 呼出速度
- Selection 获取成功率
- 空闲 CPU 占用
- 常驻内存
- 翻译首结果延迟
- UI 流畅度
- Windows / macOS / Linux 行为一致性
- 错误恢复能力

因此 V0.1 阶段不优先实现：

- 插件系统
- 复杂 OCR Provider 生态
- TTS Provider 生态
- 生词本 Provider 生态
- 大量 AI 功能

这些能力均保留架构扩展点，但不能拖慢最核心的划词翻译闭环。

---

## 7. 产品与架构关系

当前推荐的职责原则：

> **Core defines behavior, Platform provides capabilities, Translate provides services, UI renders state, App wires everything together.**

即：

> **Core 定义行为，Platform 提供系统能力，Translate 提供翻译能力，UI 负责状态展示，App 负责组装。**

在此原则下，Pot 的大部分能力都可以通过新增 Provider、Capability 或 Core workflow 实现，而无需破坏现有模块边界。
