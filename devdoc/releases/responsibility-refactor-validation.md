# 标注、窗口 Bridge 与 OCR 职责整理验证

日期：2026-10-11。Windows，本轮保持 7 个 crate；未变更配置、模型包、下载地址或可见文案。

## 本轮变化

- 标注按生命周期、画布输入、文字／焦点、参数、工具条／面板和渲染调度拆分，Registry 组合资源状态，原测试按职责迁移。
- Core 管理导出会话与请求，App 调度独立采集、纯合成及保存；专属线程结束时释放其线程局部字体和纹理缓存。UI 不再创建导出线程或轮询结果。
- Bridge 按窗口拆分，OCR 框选与 Provider 设置分别管理；具体 OCR 模型选择归 Platform。
- 更新 Workspace 说明及 Python 缓存、模型包和验证产物规则。

## 验证证据

### 自动化

- `cargo test --workspace --all-targets`：App 59、Config 24、Core 140、Platform 187、Translate 43、UI 365 项通过；另有 HTTP 子进程测试 1 项通过。112 项需要特殊环境的测试按原规则忽略，未将忽略项计为通过。日志：`target/refactor-final-tests.log`。
- `cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check` 通过。UI 测试及 Release 均完成 Slint 编译。日志：`target/refactor-clippy4.log`。
- `cargo build --release -p lexift-app` 通过，产物：`target/release/lexift.exe`。SHA-256：`6330FF04FDA1B01501017E5599BAFE1B9CD4185DFE05EA64DFDB315006F6FDC2`。日志：`target/refactor-release.log`。
- Cargo metadata 检查确认仍为 7 个 crate，Core 无内部依赖，外围 Adapter 仅依赖 Core；文档链接、`git diff --check` 和 Python 缓存／验证产物忽略规则通过。
- 导出覆盖独立采集、重复／过期请求、关闭和退出取消、合成及保存失败、保存成功与取消结果。可见文档冻结和软件效果合成沿用迁移后的测试；未重新实现绘制和撤销规则。
- 真实 ONNX 测试 `ocr::cache_tests::real_cache_and_optional_reader_lifetimes` 通过：默认自动识别不加载韩文模型，追加候选才加载，释放后缓存失效。使用本机既有已验证资源；未修改或删除资源。日志：`target/refactor-real-ocr.log`。
- 原生 STA 保存对话框取消测试通过，确认取消关闭对话框并结束观察器；日志：`target/refactor-native-save.log`。
- OCR Provider 设置渲染矩阵覆盖 12 种语言、浅深色及 1／1.25 缩放，另覆盖资源状态和说明浮层。截图：`target/refactor-render/`；检查了中文渲染和实际 Settings 页面，未调整视觉。

### 本轮实际 Release

- 从绝对路径启动并核对进程 PID、路径与产物哈希（`target/refactor-launch.json`）；本轮没有其他 Lexift 实例。
- 环境实际暴露 **1 块 1920×1080、125%（120 DPI）显示器**。标注画布和 GPU 呈现窗口正常创建；绘制矩形后显示编辑手柄，工具条收起为 80 个逻辑像素并可展开。
- 点击导出打开原生 PNG 保存对话框，Escape 取消后返回标注；完成标注后画布及工具条销毁。保存对话框及辅助窗口具备 ToolWindow 排除策略。
- Alt+T 框选可取消和重开；使用独立生成的文字夹具完成 OCR，Popup 展示 `Lexift OCR test / Hello world` 原文和后续翻译结果；框选窗口在提交后销毁。框选和工具条未改变原前台窗口，保存对话框的主动交互焦点正常。
- 托盘打开 Settings、切换 Provider、本地 OCR 详情及关闭正常；Settings 与辅助窗口的身份区分保留。最后从托盘正常退出测试实例，并关闭本轮夹具。
- 实际截图：`native-toolbar-125.png`、`native-toolbar-collapsed-125.png`、`native-drawing-125.png`、`native-ocr-fixture-popup-125.png`、`native-ocr-settings-125.png`（均在 `target/refactor-render/`）。窗口枚举和交互证据为 `target/refactor-native-*.json`。

日志、截图和临时夹具均在忽略的 `target/`，不提交截图正文或个人数据。

## 未实测范围

- 100% 原生缩放、双屏／跨屏／负坐标实机验证：当前仅一块 125% 显示器，未覆盖；对应坐标、手势、窗口命中和软件渲染由自动化覆盖。
- 本轮未逐项人工操作所有标注工具、文字编辑、参数事务、马赛克／放大镜／水印及软件呈现切换；相关回归测试通过，不能替代这些场景的人工验收。
- 未人工保存导出 PNG 到磁盘或检查全部效果的最终文件；实际验证到对话框与取消，成功保存和错误路径由自动化覆盖。
- Popup 固定／八方向缩放、历史窗口操作、实际任务栏／Alt+Tab 切换和逐帧冷启动闪帧检查未完整重测；窗口身份仅核对枚举属性与已有回归。
- 翻译实机夹具为英文到英文，确认 OCR 到 Popup 的调度及结果；未作为跨语言翻译质量或流式时序验收。
- 不引用历史截图作为本轮实机证据。
