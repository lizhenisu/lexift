# 大模型服务与翻译设置

服务提供商页使用列表＋详情管理连接、凭证及持久化模型目录。翻译设置独立选择 Provider／模型，按两者保存生成参数，共用翻译提示词；不涉及模型下载、卸载或服务启停。

## 协议

- Ollama：`/api/tags`、`/api/show`、`/api/chat`，排除云端及非文本生成模型。
- LM Studio：原生 `/api/v1/models`、`/api/v1/chat`，需要支持这些接口的版本；`store:false`，不携带历史、工具或 MCP 集成。
- DeepSeek：官方 HTTPS `/models`、`/chat/completions`，模型及思考档位来自模型发现结果。

温度和 Top-P 的“模型默认”意味着不发送字段。思考档位按能力显示，不表示精确的思考 token 数。强制思考模型明确显示要求开启。DeepSeek 思考模式禁用温度，Top-P 范围 0.95–1；非思考模式禁用 Top-P。只展示最终正文，不展示思考内容。

本机服务允许显式 HTTP／HTTPS 地址，包括局域网、公网、域名及 IPv6；地址必须为服务根地址。Ollama／LM Studio 绕过代理，全部服务禁止重定向，保留 TLS 校验。连接、发现、生成分别限制为 5／10／120 秒，不自动重试生成或回退服务。

## 开发凭证

`scripts/run-dev.ps1` 只从项目 `.env` 读取 `LEXIFT_DEEPSEEK_API_KEY`，已有进程环境变量优先，结束后恢复进程环境。应用优先使用系统凭证，没有时使用环境变量。Release 不读取 `.env`；普通配置不保存密钥。

Schema 8 保留 Schema 7 的服务／翻译解耦配置，并将既有选用模型迁入启用目录；更早的 Ollama 地址和模型继续迁移。旧 DeepL 配置与凭证不变。UI 查询由窗口持有，关闭时取消；结果验证请求版本及地址。每次生成冻结配置，不携带其他翻译历史。

## 重构前验证记录（2026-10-07）

- `cargo test --workspace --quiet`：685 项通过，99 项因原有平台或显式实机条件忽略。
- `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`、Windows `cargo build --workspace --release` 通过，包含 Slint 及 12 种语言目录编译。
- 协议回归覆盖 LM Studio 原生模型发现、Bearer Token、无历史生成、参数映射，Ollama 独立 system/user 消息、思考能力，DeepSeek 思考与采样限制，以及异常响应、截断和重定向拒绝。
- 凭证回归确认系统凭证优先，只有 DeepSeek 可使用环境变量回退；删除使用实际凭证引用，配置写入失败恢复原密钥，不影响其他 Provider。
- 使用已安装 Ollama 模型和 DeepSeek API，分别完成固定文本 `Hello, world.` 的中文翻译，未输出密钥、响应全文或思考内容。未安装服务、下载模型或修改模型权重。
- 实际 Release App 在 100%／125% 双屏检查了服务页、翻译页、Ollama 刷新、模型菜单、温度开关、滑杆及数值框。截图为 `target/llm-native-services-*.png`、`target/llm-native-model-menu-*.png`、`target/llm-native-parameters-*.png`。修复了检查中发现的下拉框点击与滑杆宽度问题；测试后恢复原配置并结束本次测试进程。
- 12 种语言、两种 DPI 的软件渲染截图在 `target/llm-settings-snapshots-final/`；模型刷新不自动选中首项、Provider／模型参数隔离及窗口任务取消有 UI 回归。

上述重构前验证未覆盖 LM Studio 实际服务；后续 Provider 管理重构的实测见下节。局域网、公网、IPv6／HTTPS 连接只有地址及客户端规则覆盖，未逐一实网验证。未重新进行划词／Alt+X／图片剪贴板的人工回归，未人为等待完整 120 秒生成超时；这些场景不能按实测通过报告。模型质量不由协议测试保证。

接口参考：[Ollama 思考能力](https://github.com/ollama/ollama/blob/main/docs/capabilities/thinking.mdx)、[LM Studio 模型发现](https://lmstudio.ai/docs/developer/rest/list)、[LM Studio 生成](https://lmstudio.ai/docs/developer/rest/chat)、[DeepSeek 生成参数](https://api-docs.deepseek.com/api/create-chat-completion/)。

## Provider 管理重构（2026-10-07）

- DeepL、Ollama、LM Studio、DeepSeek 共用凭证控件，提供查看、复制、编辑、删除。DeepSeek 环境变量凭证单独标记，不能通过删除按钮修改环境变量或 `.env`；删除系统覆盖后可继续使用环境凭证。
- 服务可停用，停用后禁止新翻译，不替换 Provider；已冻结的请求保持原设置。连接及高级设置需显式应用，可取消；凭证独立保存。
- 目录区分发现结果、用户启用、服务可用状态及确认过的协议／地址。新发现模型不自动启用，暂时缺失模型不自动删除。翻译页直接读取保存的启用目录，切换 Provider 无需再刷新，并保留各自选择和参数。
- 搜索及筛选支持文本、嵌入、未知能力及不可用模型；删除只移除本地目录记录，重新同步可以再次发现，不操作服务端模型权重。
- 支持原生、OpenAI Chat Completions 和 OpenAI Responses；没有 Anthropic 适配。每个协议保留独立 API Base URL、认证方式、请求头引用。兼容 Base URL 已包含部署路径，例如 `http://127.0.0.1:1234/v1`，只追加 `/models`、`/chat/completions` 或 `/responses`，不会再加 `/v1`。
- 原生服务地址和高级 API 地址均支持用户明确指定的网络服务；DeepSeek 默认仍为官方 HTTPS。服务接受模型发现请求不等于支持所选生成接口，应单独测试翻译模型。
- 模型发现不生成文本，连接检查只读取模型端点。模型测试仅发送固定测试文本。原生 LM Studio 返回的 `key` 与 `loaded_instances[].id` 可用于确切关联兼容接口 ID，不凭显示名称猜测别名；无法验证的能力不启用。
- 思考能力由模型及协议共同决定；兼容协议缺少能力依据时禁用思考设置。Responses 仅返回完整 assistant 的 `output_text`，拒绝未完成输出、工具调用及拒答，忽略 reasoning。
- 自定义请求头值先留在所属窗口草稿中，应用时与连接设置一起提交；多个凭证写入或配置保存失败会恢复此前凭证。测试草稿可使用未保存值，不写系统凭证。取消／切换 Provider／关闭窗口释放草稿。
- 所有请求头值保存至系统凭证存储，普通配置只保存名称和引用。名称按大小写不敏感处理，拒绝换行、连接管理头及重复项；自动 API Key 和自定义认证头不能并用。
- 查看和编辑的明文只属于当前凭证访问代次。关闭窗口、切换 Provider、取消或失焦会失效；晚到的复制操作在写剪贴板前再次检查代次。

### 本轮验证

- 最终 `cargo test --workspace --quiet`：693 项通过、100 项显式忽略；单独执行的 LM Studio 实机协议测试也通过。`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings` 及 Windows `cargo build --workspace --release` 通过，包含 Slint 编译。
- 临时 LM Studio `http://127.0.0.1:52341`：原生、Chat Completions、Responses 均通过实际模型发现和固定文本中文生成；嵌入模型未进入翻译候选。不改变默认端口、不安装服务、不下载或删除模型。
- 自动化覆盖 Schema 7→8、目录启用与离线保留、连接变化重新确认、翻译下拉列表即时联动、各 Provider 凭证操作、请求头事务失败恢复，以及兼容请求路径、认证、参数省略和最终输出校验。
- 软件渲染截图：`target/provider-settings-snapshots/`，覆盖全部 12 种语言、100%／125% 缩放的 Provider、翻译和高级配置页。
- 实际 Release 窗口在 100%／125% 双屏完成模型同步、目录显示、高级配置展开、翻译模型下拉列表及 DeepSeek 凭证页检查；截图为 `target/provider-native-{services,catalog,advanced,translation,model-menu,deepseek}-{100,125}.png`。首次同步无需先应用默认连接，翻译下拉列表无需再次联网。测试后恢复原配置，未改变模型文件或停止用户服务。
- 本轮未重新实测 DeepL、Ollama、DeepSeek 的真实翻译请求；它们由自动化回归及上述历史实测记录覆盖。局域网、公网、HTTPS、认证网关以及划词／Alt+X 的人工回归仍未覆盖。
- 本轮不新增依赖或 crate，不实现模型权重管理、手动模型 ID、任意 Provider 创建或图像生成。

## Provider 页面布局重做（2026-10-08）

按 `model_provider/` 中的 Cherry Studio 截图重新组织页面，保留 Lexift 的设计值、凭证和协议边界。

- Provider 列表与详情各自滚动；详情不再包在大卡片中。名称、连接状态和开关共用顶栏，密钥操作及检测紧贴输入框，地址右侧的齿轮打开请求配置。
- 主页只显示已启用模型，搜索和能力筛选按需展开。减号停用模型并保留目录及翻译参数；目录移除和固定文本测试置于更多菜单。空状态、窄窗口和长名称有独立展示。
- 同步模型打开窗口内的模态目录，发现结果使用加／减启用或停用，实时更新翻译模型列表；嵌入模型保留能力说明，不能启用用于翻译。未应用连接时禁止同步正式目录，检测可使用当前草稿。
- 请求配置使用独立模态弹窗，集中协议、地址、认证和请求头。确定只合入页面草稿；取消恢复打开弹窗前的草稿，包括此前未应用的请求头。主页只有存在修改时显示固定的取消／应用栏。
- 弹窗关闭恢复触发控件焦点；Escape 先关闭更多菜单，再关闭弹窗。目录关闭、页面离开和 Provider 切换使所属查询失效。临时明文继续遵守凭证生命周期，不创建额外原生窗口。

### 布局验证

- 最终 `cargo test -p lexift-ui --quiet`：333 项通过、10 项显式忽略；`cargo test -p lexift-app llm_credentials --quiet`：3 项通过。`cargo fmt --all -- --check`、`cargo clippy -p lexift-ui --all-targets -- -D warnings` 和 Windows `cargo build --workspace --release` 通过，包含 Slint 及 12 种语言目录编译。
- UI 回归覆盖嵌套草稿取消／确认、未应用连接阻止同步、模型启停与翻译选择联动，以及 Escape 关闭和查询取消。
- `target/provider-redesign-snapshots/` 包含 12 种语言、100%／125% 的主页面、请求配置、模型目录、搜索／筛选、空状态及窄窗口截图；联系表仅用于逐组检查，单张截图保留原尺寸。
- 实机截图为 `target/provider-redesign-native-*-{100,125}.png`。使用临时 LM Studio 完成目录同步和连接检测，检查主页面、请求配置、目录弹窗、搜索／筛选、草稿操作栏、翻译模型列表和环境凭证的悬停状态。环境凭证截图使用仅注入测试子进程的无效占位值，不向 DeepSeek 发请求，不读取或修改用户凭证。
- 实机检查后恢复原配置并退出本次测试进程，模型文件未变化。其他 Provider 的实际联网翻译、局域网／公网连接及划词／Alt+X 未在本次布局任务中重新实测。

## Provider 交互与模型检测（2026-10-08）

本节替代上节的模型更多菜单及旧连接检测交互。

- 主页和同步弹窗均移除省略号、目录移除和行内测试菜单；保留能力标记与加／减。弹窗右上角关闭按钮没有悬停提示，仍有可访问名称和键盘操作。
- 四个 Provider 使用随程序打包的 SVG，绿点只表示已保存的启用状态；连接草稿不会改变绿点。按钮的同步、检测、凭证操作状态分别管理。图标按钮的鼠标背景与键盘焦点描边分开，关闭齿轮弹窗后保留焦点描边，不保留悬停背景。
- 测试连接打开模型检测弹窗，直接读取已启用目录，不刷新模型或改变翻译选择。优先选用当前翻译模型，否则仅在弹窗内预选首个可检测模型。不可用模型保留原因，嵌入专用模型不参与检测。
- 检测冻结页面连接草稿、请求头及模型列表，使用已保存凭证，不保存草稿。固定发送 system `Reply with OK only.` 和 user `Connection test`，不使用正文、翻译提示词或历史；温度与 Top-P 省略，可关闭的思考关闭，强制思考使用模型提供的合法档位。
- 三种大模型 Provider 均实际生成短文本。检测所有模型需先确认，仅覆盖已启用模型，串行执行；单项沿用 120 秒生成超时，失败后继续。结果显示成功、失败、跳过及耗时，任意适配器错误文本不会直接显示，以免泄露凭证。
- 取消、Escape、页面关闭或 Provider 切换取消所属请求及后续队列。完成回调核对页面、版本及冻结连接；结果仅留在当前页面，关闭后释放。同步仍只发现模型。DeepL 保留原有凭证交互。
- 没有新增 Port、配置 Schema、依赖或 crate；12 种语言均补齐检测文字。

### 本轮验证与证据边界

- 最终串行 UI 测试：333 项通过、10 项显式忽略；凭证回归 3 项通过。`cargo fmt --all -- --check`、`git diff --check`、UI Clippy（all-targets、拒绝 warnings）及 Windows workspace Release 构建通过，包含 Slint 编译。
- 并行 UI 测试曾有一项既有划词工具条 DPI 时序断言失败（332 通过、1 失败）；该项单独执行及最终全套串行执行通过。本次不修改划词代码，保留此时序不稳定记录。
- 检测回归覆盖三家 Provider、固定测试文本、参数省略、强制思考、草稿连接、单项／批量、部分失败、跳过、空目录、重复开始、取消及过期回调；确认不会修改翻译设置和正式目录。凭证错误详情使用白名单反馈。
- `target/provider-interaction-snapshots/` 保存 312 张软件渲染截图，覆盖 12 种语言、100%／125%、主页面、请求配置、同步目录、检测单项／批量／运行中／结果及窄窗口。
- 最终 Release 在实际 100%／125% 双屏使用临时 LM Studio `http://127.0.0.1:52341` 和既有 `hy-mt1.5-1.8b` 模型完成单模型与批量生成检测。截图为 `target/provider-interaction-native-*-{100,125}.png`；检查了图标、绿点、菜单删除、同步按钮独立状态、齿轮关闭及连接草稿取消。最终示例：100% 单项 366ms、125% 批量单项 493ms；这些时间仅是当次请求记录，不代表性能承诺。
- 测试后恢复用户原配置并退出测试进程，未下载、修改或删除模型，未停止用户服务。DeepSeek 凭证页面使用测试子进程中的无效占位环境变量，不读取用户密钥、不向 DeepSeek 发请求。
- 本次没有重新实网检测 Ollama、DeepSeek、DeepL，也没有实测局域网／公网服务、完整 120 秒等待或划词／Alt+X；相关失败和取消由自动化覆盖。原生模型响应很快，运行中布局另由固定软件渲染夹具验证。

### 标识来源

Ollama、LM Studio、DeepSeek 标识来自 [Cherry Studio Provider SVG](https://github.com/CherryHQ/cherry-studio/tree/main/packages/ui/icons/providers/light)，DeepL 来自 [Simple Icons](https://github.com/simple-icons/simple-icons/blob/develop/icons/deepl.svg)。仅用于识别对应服务；保留原有品牌图形，调整显示色或视口以适应列表。
