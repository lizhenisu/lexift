# OCR Provider 与资源管理验证

日期：2026-10-10～11。构建版本：0.0.1-rc.9，Windows x64。

## 自动化

- App 54、Config 22、Core 131、Platform 178、UI 367 项通过，共 752 项；既有 ignored 测试不视为通过。日志：`target/ocr-provider-tests.log`、`target/ocr-provider-final-tests.log`、`target/ocr-provider-ui-final.log`。
- 覆盖旧配置默认启用和关闭 roundtrip、按 Provider 排队与失败恢复、未保存连接草稿隔离、资源操作独立版本、取消后重查、迟到进度及结果拒绝、下载完成提示、无法检查时清除旧就绪状态、禁用后不产生 OCR 命令。
- Slint 编译通过，12 种语言 × 浅深色 × 100%/125% 共 48 张软件渲染图，包含窄窗口；文件：`target/ocr-provider-screenshots/`。检查三组导航、OCR 详情和返回原 Provider 的连接草稿。新增文案使用全部 12 种现有目录。
- 普通删除、目录范围、非法待删除版本、文件占用失败保留标记以及解锁重试通过。真实 ONNX Runtime 测试在独立副本加载 DLL、释放 Session 后仍无法删除 DLL，转为等待重启；第二个独立进程启动清理成功。日志：`target/ocr-provider-real-removal.log`。未删除用户实际使用的 OCR 目录。
- 真实运行库测试使用 `LEXIFT_OCR_RESOURCE_FIXTURE` 指定只读源版本目录，并复制到 `target/ocr-provider-removal-fixture`；两个 ignored 测试须分两次独立 `cargo test` 执行，第二个测试清理本次副本。这不是客户端路径覆盖机制。
- `cargo fmt --all`、相关 crate 的 `cargo clippy --all-targets -- -D warnings` 通过，日志：`target/ocr-provider-clippy-final.log`、`target/ocr-provider-ui-clippy-final.log`。

## 原生与交付记录

- Windows Release 构建通过：`target/ocr-provider-release-delivery.log`。产物 `target/release/lexift.exe`，SHA-256：`b59e8a9af171353fd8552ade09ae0657281acbccb6c63cc343aea080ac3dc6d9`。
- 从桌面 Shell 启动上述物理路径，进程路径及 hash 记录：`target/ocr-provider-native-launch.json`。用户实际 OCR 目录保持完整，资源页面本地检查显示 v1、下载 37.9 MiB、本地占用 55.4 MiB。
- 主屏 2560×1440 / 125%，副屏 1920×1080 / 100%。实际 Settings 在两屏之间移动，三组导航、语言控件、资源明细、开关及删除按钮均正常。截图：`target/ocr-provider-screenshots/actual-provider-125.png`、`actual-provider-100.png`、`actual-provider-final-125.png`。
- 删除入口打开窗口内确认，说明完整模型、字典及运行库范围；Escape 取消后资源仍就绪。截图：`actual-delete-confirm-125.png`。未在用户真实目录确认删除。
- 实际关闭 OCR，立即出现保存反馈且绿点消失；Alt+T 仅创建错误 Popup，显示本地化“服务已停用”，没有框选 HWND。随后恢复原启用状态。截图：`actual-provider-disabled-100.png`、`actual-disabled-shortcut-popup.png`、`actual-provider-restored-100.png`。普通配置只核对 `ocr_enabled` 字段，不保存配置正文到验证日志。
- 使用自建纯测试文字窗口执行 Alt+T，两块遮罩尺寸分别精确覆盖显示器且维持辅助窗口样式；前台保持测试窗口。松手后真实 OCR 正文正确进入 Popup；当前模型服务不可连接，翻译显示对应错误并保留原文，不能记为翻译成功。截图：`actual-ocr-popup-125.png`。再次 Alt+T 后 Escape 关闭所有遮罩。测试宿主已销毁。
- HWND 记录显示 Settings 正常应用身份，主窗口、Popup 及 OCR 遮罩仍为辅助身份；本轮没有重新独立检查 Shell 的实际 Alt+Tab 列表或任务栏缩略图。未修改窗口策略。
- 最终已运行新版并保留 Settings 便于查看；未暂存、提交或推送。

## 验收边界

- 固定模型资源包、清单及下载 URL 未改动；生产资源 Release 尚未发布。本轮下载取消、损坏与原子启用继续由本地 HTTP 回归验证，不能宣称生产下载入口可用。
- 本轮不新增 OCR 服务、逐模型下载、子进程、第三方依赖或 workspace crate。
- 用户实际资源保留；真实 DLL 占用与启动删除使用隔离副本验证，未对个人资源进行破坏性演练。

本轮未实测：完整 App 重启后的设置回填（由配置 roundtrip 覆盖）、用户真实目录删除（由隔离真实运行库副本覆盖）、生产资源下载、当前不可连接服务的译文成功路径、暗色和全部语言的原生截图（由软件渲染覆盖）。没有更改安装包或便携包。

## 2026-10-11 OCR 详情页改版

- 独立 OCR Provider 视图复用公共设置控件，使用识别设置与本地资源两张卡片；状态徽标、元信息、结构化模型明细、下载进度及反馈区分层呈现。
- 就绪隐藏下载入口；删除改为带 SVG 的低强调操作，禁用有视觉差异，并保留窗口内二次确认。主页面只显示 MiB，下载确认仍包含精确字节数。下载完成说明不使用错误色。
- UI 回归 367 项通过、10 项按既有规则跳过；App 回归 54 项通过。设置菜单键盘操作、弹窗焦点和切页草稿隔离通过。专项补充结构化明细、精确/紧凑大小、下载进度和成功/失败反馈检查。
- 12 种语言 × 浅深色 × 100%/125% 产生 48 张软件渲染图；另有未安装、检查、下载、损坏、删除、等待重启及长 HTTP 错误共 7 个状态快照。截图在忽略的 `target/ocr-provider-redesign/`。已查看中文浅色、德文窄窗口深色、下载、待重启及长错误画面，无可见重叠或截断。
- 本轮没有改动资源 URL、配置、模型或 Core Port，公共下载 404 仍未验收。
- 最终格式、Slint 编译、UI/App Clippy（`-D warnings`）和 Windows Release 构建通过；Release SHA-256：`F7D0B4D047E66718FE01B440D78FF9B8B856ACB03A55B1DC97D7E8607FF5B522`。
- 实际启动 `target/release/lexift.exe` 并核对进程路径。普通窗口和最大化下卡片正常，滚动可访问底部操作；资源状态 Ready，显示 52.1 MiB，四组大小与实际清单一致。点击删除打开原二次确认，Escape 取消后删除按钮获得键盘焦点，未删除资源。截图：`actual-provider-page.png`、`actual-resource-card.png`、`actual-delete-confirm.png`，均位于上述忽略目录；测试后恢复原窗口大小。
- 本轮实际页面核对使用当前显示器，未重新完成双屏 100%/125% 全部实机组合；双比例、多语言和深色覆盖来自软件渲染，不替代未执行的实机操作。没有执行真实资源删除或生产下载。
