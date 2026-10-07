# Ollama 首版实现记录（历史）

> 当前配置、地址支持、提示词和模型参数已迁移至[大模型服务与翻译设置](llm-providers.md)。以下保留首版设计和验证记录，不代表当前限制。

# 本机 Ollama 翻译

## 使用方式

在设置的 **Service providers → Translation providers** 中选择 Ollama，填写本机服务地址，点击“测试连接／刷新模型”，明确选择一个模型，再点击“应用”。默认地址为 `http://127.0.0.1:11434`，不自动选择模型。

Provider 选择沿用已有即时保存机制；地址和模型通过“应用”一起保存。未选择模型也能保存 Ollama Provider，发起翻译时会提示先选择模型。编辑地址会清除草稿中的模型和连接结果；不会清除磁盘上已经保存的配置。服务离线或模型被移除时不会切回 DeepL。

模型下载、删除、服务启停和模型驻留均由用户自己的 Ollama 管理。Lexift 不执行这些操作，也不提供模型卸载承诺。翻译质量和格式遵循情况取决于用户选用的模型。

## 模块边界

- Core：`OllamaConfig`、地址校验、模型兼容性与连接状态类型、只读 `OllamaServicePort`。`TranslatorPort` 不变。
- Translate：独立 HTTP 客户端、列表和详情读取、原生 chat 请求与响应适配。
- Config：Schema 6 持久化地址和模型；Schema 1–5 缺少新字段时使用默认地址与空模型，保留原 Provider 和凭证引用。
- UI：复用设置卡片、下拉列表、反馈和滚动；查询归设置窗口所有，编辑地址、重复刷新和销毁窗口会取消旧查询。结果同时检查窗口弱引用、版本和地址。12 种界面语言均提供对应文案。
- App：注入服务查询执行器，在后台运行 Core Port；翻译入队前冻结 Provider、地址和模型，后台再读取 DeepL 凭证，避免在 UI 线程访问凭证存储。

## 本机请求约束

仅接受带显式有效端口的 HTTP 回环 origin，可带一个根路径 `/`。支持 `localhost`、IPv4 回环地址及 `[::1]`；localhost 被规范化为 `127.0.0.1`，不依赖 DNS。拒绝用户信息、查询、片段、非根路径、非回环主机及 URL 解析歧义。

客户端禁用系统代理、重定向与自动重试。连接超时 5 秒，完整模型查询预算 10 秒，翻译总预算 120 秒；DeepL 仍使用原来的客户端与超时。

刷新只调用 `GET /api/tags` 和 `POST /api/show`，不加载权重或发送待译文本。只有报告 `completion` 能力且没有 `remote_model`／`remote_host` 标记的模型可以选择；嵌入、云端、能力不明模型显示具体原因。发送正文前重新检查模型详情，防止已保存模型被替换后继续使用旧兼容性结果。

每次翻译通过 `POST /api/chat` 发送两个独立消息，明确 `stream:false`，不携带会话历史，不覆盖模型模板，不设置 `keep_alive`、推理计算参数或云端回退。提示要求只输出译文并保留换行和空行。仅接受完成原因为 `stop` 的非空 `message.content`；思考字段被忽略，工具调用、截断和异常响应作为失败处理。日志不记录正文或完整服务响应。

上游接口：[模型列表](https://docs.ollama.com/api/tags)、[模型详情](https://docs.ollama.com/api/show)、[Chat](https://docs.ollama.com/api/chat)、[云端标记类型定义](https://github.com/ollama/ollama/blob/main/api/types.go)。

## 回归与复验

自动化覆盖严格地址校验、代理绕过、重定向拒绝、能力与云端过滤、请求格式、思考隔离、异常结果、离线、未配置模型、旧配置迁移、无 DeepL 凭证、配置快照和窗口查询取消。软件渲染截图覆盖 12 种语言、100%／125% DPI 及窄窗口，截图输出到 `target/ollama-settings-snapshots`。

```powershell
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace --release

# 仅对用户已安装模型执行显式实机验证，不安装或下载模型。
$env:LEXIFT_TEST_OLLAMA_MODEL = '<已安装模型的完整名称>'
cargo test -p lexift-translate installed_local_model_smoke -- --ignored --nocapture
```

2026-10-07 已使用本机现有 `huihui_ai/hy-mt1.5-abliterated:1.8b-q3_K` 验证中英双向、多行和连续翻译。初始提示下模型曾合并跨行的两个词；补充逐行保留要求后该用例通过。这是提示约束，不能据此保证任意模型都严格保留格式。

本轮 Workspace 测试通过 674 项，97 项按原有平台／显式实机条件忽略；Workspace Clippy（全部 targets，warnings 作为错误）和 Windows Workspace Release 构建通过，包含 Slint 编译。

Windows 实际 Release App 在 100%／125% 双屏验证了无 DeepL 凭证选择 Ollama、刷新模型、明确选择模型、应用保存及输入翻译。启动前剪贴板放置图片后，划词工具条出现时 Ctrl+C／Ctrl+X 仍正常。划词翻译及 Alt+X 均可进入 Ollama 请求链路；该已安装的 1.8B Q3 模型在重复短词组测试中出现过空译文、原样返回及无关输出，空结果被正确显示为失败，不将这些用例记为译文质量通过。输入窗口的中英多行用例曾获得正确译文，不能据此保证模型的稳定质量。

实机截图保存在 `target/ollama-native-*.png`。测试后恢复了原配置与剪贴板；Lexift 测试进程结束后 Ollama 仍可查询，模型目录文件清单、尺寸和修改时间未改变。未安装服务、下载模型或调整资源驻留。测试进程以窗口关闭后终止兜底结束，未单独验收托盘正常退出流程。

服务离线、模型缺失、云端模型、异常响应和旧查询晚到由自动化覆盖；未在用户服务上删除模型或人为等待 120 秒超时。重复实机测试中，Alt+X 请求在 12 秒截图时仍可能处于翻译中，因此未验收该模型稳定的完成时延；部分无关输出还暴露了既有翻译结果显示裁剪，本轮未扩大修改范围。未主动卸载驻留模型来制造首次冷加载，负坐标没有实机验证；不将未覆盖场景作为已通过报告。
