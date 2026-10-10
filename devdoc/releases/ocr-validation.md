# 本地 OCR 实施验证

日期：2026-10-10。构建版本：0.0.1-rc.9。Windows x64，两个显示器：主屏 2560×1440 / 125%，副屏 1920×1080 / 100%，副屏位于主屏右侧。

## 自动化结果

- 全 workspace 回归通过：App 54、Config 21、Core 121、Platform 172、Translate 43、UI 363；日志 `target/ocr-workspace-tests-final.log`。既有 ignored 实机测试未视为通过。
- 补充任务替换测试后 App / Config / Core 再次通过，Core 122；日志 `target/ocr-app-core-config-tests-final.log`。
- 最终 Platform 回归 171 通过、86 ignored（完整资源测试改为显式运行）；日志 `target/ocr-platform-tests-final.log`。
- 真实 PaddleOCR + Windows ONNX Runtime：12 个语言配置分别执行对应模型，覆盖简繁中文、日文、英文、拉丁重音文字、韩文、空图及超过 960px 的检测缩放；日志 `target/ocr-inference-test-final.log`。不是 mock 识别验收。
- 实际完整 39,784,517 字节 ZIP 经本地 HTTP 下载、解包与全部文件校验通过；日志 `target/ocr-full-archive-test.log`。另有损坏、保留原版本、取消停滞响应及未校验运行库拒绝加载测试。
- Slint 实际像素回归覆盖 100% / 125%：选区内清晰、四周变暗；注册表回归覆盖窗口确实显示、反向拖动和旧会话。日志 `target/ocr-ui-tests-final.log` 及 workspace 日志。
- `cargo clippy --workspace --all-targets -- -D warnings` 通过，日志 `target/ocr-clippy-final.log`。Release 构建日志 `target/ocr-release-build-final.log`。

## 原生验证

验证仅使用自建测试图片窗口，不采集个人文档或记录识别正文到日志。临时探针与截图保留于忽略的 `target/ocr-native/`，HWND、样式、DPI、尺寸及步骤记录在 `record.json`。

实施时修复的问题：Slint 独立子元素默认居中造成遮罩偏移；提示宽度循环依赖；遮罩未实际 show；关闭 Settings 后延迟焦点事件误取消遮罩；Winit 全屏路径主动激活 HWND。最终使用原生无边框物理尺寸铺满显示器，避免进入会主动激活的 fullscreen 路径。

### 完成的实机步骤

- 首次缺失资源时打开 Settings 内确认，显示 37.9 MiB / 39,784,517 bytes；取消后未启动截图。截图 `first-download-confirmation.png`。
- 两屏 HWND 客户区分别严格为 2560×1440 和 1920×1080，未覆盖后再采集。两屏开始框选时前台 HWND 均保持原测试输入窗口。
- 两屏均完成松手 → 本地识别 → DeepL 自动翻译，原文包含简繁中文、日文、英文，截图 `popup-0.png`、`popup-1.png`。
- 两屏 Escape、右键、重复 Alt+T 均关闭所有遮罩。跨屏四方向拖动全部完成识别翻译，截图 `cross-screen-direction-0..3.png`、`cross-screen-popup-0..3.png`。
- 独立检查实际 Alt+Tab：列表只有测试宿主及原应用，没有 OCR / Lexift 辅助窗口；截图 `alt-tab-with-ocr.png`。
- 任务栏自动隐藏时 Win+T 截图为黑色，未将其作为通过证据。随后通过实际开始菜单展开任务栏，确认没有 Lexift 按钮，且 OCR HWND 仍存在；有效证据 `start-and-taskbar-with-ocr.png`。
- 便携包内程序单独启动，子进程 PATH 仅保留 Windows / System32，没有 Python 搜索目录；完成双屏遮罩、不抢焦点及真实 OCR → 翻译。截图 `portable-popup-no-python-path.png`，步骤 `portable-record.json`。这不等同于干净 Windows 系统测试。

### 构建产物

`target/ocr-package/` 已生成未签名 NSIS 安装包、便携 ZIP 和 `SHA256SUMS.txt`，日志 `target/ocr-package.log`。未执行安装、卸载或上传。

## 资源与发布边界

已生成 `target/ocr-resources/lexift-ocr-v1-windows-x86_64.zip`，SHA-256：`3104be8e91fc523d917b089cd640cd2cee4a85224f93b2d299d172738b3a6de9`。模型、字典、Windows CPU 运行库及许可均在版本清单中固定大小和校验值。客户端运行不调用 Python；Python / Paddle2ONNX 只用于开发端资源导出。

**尚未上传资源 Release。** 生产下载 URL 尚不能作为已验收入口；发布须另行得到用户确认。当前真实推理使用本地校验后的同一资源包，下载流程使用本地 HTTP 夹具。

## 未实测范围

- 当前双屏没有负坐标显示器，负坐标裁切由纯函数回归覆盖。
- 实机翻译使用当前 DeepL；LM Studio 流式 OCR 结果交接未单独实测，沿用既有翻译链路回归。
- 没有无 Python 的干净 Windows 虚拟机；安装版实际安装、卸载及干净系统依赖验收未完成，不能由构建成功替代。
- 任意复杂版面、旋转文字、垂直排版不作识别质量保证；首版使用轻量检测及水平文本裁切，检测最长边 960，选区最多 4000 万物理像素。
- ONNX 同步运行中的取消在阶段边界检查，过期结果由 Core 版本校验丢弃，不将部分结果提交翻译。
