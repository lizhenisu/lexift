# 本地 OCR 框选翻译

## 使用

- 默认快捷键 **Alt+T**，可在 Settings → 热键设置修改；与划词翻译、标注快捷键互斥。
- Settings → 服务提供商 → OCR → 本地 OCR（PaddleOCR）管理识别语言及完整资源包。首次缺少资源时自动定位此处，确认下载完成后再次按快捷键。
- OCR 开关即时保存，默认开启；关闭保留语言、快捷键和资源，只禁止之后启动的识别请求。OCR 不属于翻译模型选择器。
- 先冻结所有显示器，再显示独立遮罩。拖动框选，松手识别；无面积点击继续等待。Escape、右键或再次按快捷键取消。
- OCR 原文及当前服务生成的译文显示在既有 Popup 中。识别失败或空结果不翻译；翻译失败保留可编辑原文。
- 标注进行中拒绝框选并提示退出标注；框选不使用标注 Registry。
- 识别语言独立于翻译语言；新安装默认自动检测，旧配置保留原选择（缺失字段仍跟随界面语言）。识别完全本地执行；翻译仍遵循选中服务的联网规则。

## 模块与生命周期

Core 定义 `OcrState`、版本化事件／命令及 `OcrPort`。Platform 提供一次性 GDI 屏幕采集、校验下载及 ONNX CPU 推理；App 调度阻塞工作，UI 管理临时遮罩。冻结图像和正文不写日志或普通配置。

每次请求递增 generation。关闭所属 Popup、取消或替换请求后，旧识别结果不能启动翻译。遮罩按需创建，结束即销毁；所有遮罩遵循辅助窗口策略。推理 Session 在最后一次任务结束后空闲两分钟释放，再次使用按需创建。

屏幕和选区坐标均为物理像素。不同显示器之间的空隙补白。检测图最长边缩至 960，映射回原图裁切识别，DB 连通区域展开、CTC 解码并按阅读顺序保留换行。单次选区最多 4000 万像素，过大选区明确报错。

## Provider 与资源管理

统一服务身份 `ServiceProviderId` 区分翻译服务和 `PaddleOcr`，不扩充翻译 `ProviderConfig` 的序列化值。OCR 启用字段 `ocr_enabled` 缺省为开启，通过 Provider 定向变更在最新配置上合并；保存队列按 Provider 区分，连接草稿不参与保存。快捷键仍在热键设置，语言迁至 OCR Provider 详情。

资源管理使用独立请求编号与状态（检查、下载、删除），不复用框选 generation。基础包与韩文扩展分别展示版本、压缩下载大小、已安装大小及操作。基础包包含检测、多语言 Small、字典和运行库；韩文扩展只包含识别模型、字典及许可。扩展清单声明兼容的基础版本，缺少扩展不影响基础 OCR 就绪。页面只检查本地文件，下载需确认，不因选择语言自动联网。

两种资源操作共用串行协调；框选或识别中不能删除，旧进度和结果不能覆盖当前操作。删除韩文扩展保留基础能力；删除基础包同时清理依赖扩展，确认文案明确范围。韩文不含 DLL，可在释放其会话后独立删除。

删除需窗口内确认，目录限定为已验证的 OCR 专属版本目录，不跟随资源目录的链接或 junction。未加载运行库时释放推理会话并立即删除；已加载时，ONNX Runtime 的进程级 DLL 不能随 Session 释放，因此原子保存待删除标记，立即禁用资源，显示等待重启。下次启动先清理再允许加载。清理失败保留标记、反馈实际文件状态并允许重试；待删除期间禁止识别和下载。普通配置、凭证、其他应用文件不受删除影响。

验证记录见 [OCR Provider 与资源管理](../releases/ocr-provider-validation.md)。

## 框选窗口基础

原生窗口准备、首次绘制托底及共用手势边界见 [屏幕区域选择基础](region-selection.md)。取消监听与外观准备已分离；首次显示验证见 [首帧记录](../releases/ocr-first-frame-validation.md)。

## 资源构建与发布

客户端不调用 Python。基础资源位于 `%LOCALAPPDATA%\Lexift\ocr\v1`，韩文扩展位于同级 `korean-v1`，安装版与便携版共用。产品尚未发布，基础包统一从 v1 起算；此前本机试验包不做迁移，安装时保留旧试验目录备份并使用重新校验的 v1 包。

`scripts/build-ocr-resources.py` 在开发环境使用官方 Paddle2ONNX 导出固定 Hugging Face 官方模型修订，生成模型、字典、ONNX Runtime Windows x64 DLL、许可和第三方声明的 ZIP，以及编译进客户端的 `assets.json`（版本、精确字节数、每文件与 ZIP 的 SHA-256）。输入模型修订记录在清单中。

导出环境：Python 3.12、PaddlePaddle 3.1.1、Paddle2ONNX 2.0.2rc3、ONNX Runtime 1.22.0。首版拆包复用已固定的官方 PP-OCRv6 Small、韩文 v5 mobile 及 v5 mobile 检测模型，重新验证 batch 1/2/4 和输出字典契约。运行库保持相同版本，不在进程内切换 DLL。

开发端可执行：

```text
python scripts/build-ocr-resources.py --runtime-dir <Windows onnxruntime/capi 目录>
```

实际 ZIP 输出至忽略的 `target/ocr-resources-v1/`，分别为 `lexift-ocr-v1-windows-x86_64.zip` 与 `lexift-ocr-korean-v1-windows-x86_64.zip`，不进入 Git。清单分别为 `assets.json` 与 `korean-assets.json`。下载只启用全部验证成功的目录，采用 staging → version 原子重命名。取消中断网络 Future，损坏或失败不覆盖已有版本；DLL 仅从校验后的绝对路径加载。

**资源 Release 尚未发布。** 清单中的生产地址是待发布目标，不表示该地址当前可下载；发布资源前须另行获得用户确认。未发布阶段以本地 HTTP 夹具验证下载流程，不能称生产下载已验收。

## 验证记录

本轮记录见 [OCR 实施验证](../releases/ocr-validation.md)。识别测试图片不随源码保存。真实推理测试需先构建资源，并在本地忽略目录（例如 `target/ocr-test-images/`）准备 `common.png`、`latin.png` 和 `korean.png`，通过 `LEXIFT_OCR_TEST_IMAGES` 指定目录。图片需覆盖测试断言中的中／日／英、拉丁文字及韩文内容；缓存生命周期测试仅使用 `common.png`：

```powershell
$env:LEXIFT_OCR_TEST_RESOURCES = (Resolve-Path target/ocr-resources-v1/installed/v1).Path
$env:LEXIFT_OCR_TEST_IMAGES = (Resolve-Path target/ocr-test-images).Path
cargo test -p lexift-platform real_paddle_models -- --ignored --nocapture
```

上述环境变量仅由测试读取，不是客户端资源路径或下载地址覆盖机制。

## 自动检测

配置沿用 `ocr_language`：`auto` 为自动，缺失为跟随界面，其他合法代码为人工选择；不改变 Schema 版本。新安装默认值由 `Settings::default` 提供，已有文件反序列化仍保留 None。UI 索引与配置代码由 Core 显式映射。

`OcrRecognitionStrategy` 在快捷键请求时冻结语言、韩文自动识别设置和资源版本。默认自动模式只检测一次文字区域，每行仅调用多语言 Small；韩文扩展卡片内的“加入自动检测”（`ocr_korean_auto`）默认关闭，扩展验证就绪后才能开启，沿用定向保存和失败回滚。标签后的圆圈问号通过悬停或键盘聚焦显示共享 Settings 提示，说明额外韩文识别会增加耗时及内存占用；开关禁用时仍可查看，问号不提交设置。开启后执行已校准的双候选逻辑：CTC 保留字符概率与空间位置，只以可靠韩文证据替换对应范围，其他文字优先通用模型。

手动韩文或跟随韩文界面只执行韩文识别，不执行 Small；扩展缺失时定位扩展下载确认，不截图、不降级。人工其他语言仅使用通用模型。翻译源语言自动检测独立于 OCR 策略。

校准采用字符编辑距离对齐标签与模型输出，按固定概率区间聚合、Beta 平滑和单调回归，参数写入 `calibration.json`，不包含正文。生成图片、标签及候选仅放在忽略的 `target/ocr-auto-corpus`；`scripts/validate-ocr-auto.py` 分别生成校准及独立验收样本。真实推理 corpus 测试需额外设置 `LEXIFT_OCR_CORPUS`，识别正文不输出到日志。

旧双候选校准与验收背景见 [自动检测验证](../releases/ocr-auto-validation.md)；拆包、速度及资源生命周期的当前记录见 [韩文扩展与性能验证](../releases/ocr-extension-validation.md)。

## 推理与内存预算

下载启用及推理会话首次使用前完整校验 SHA-256。同一有效会话缓存校验结果，后续请求仅比较版本目录、文件大小和修改时间指纹；资源变更、下载、删除或两分钟释放使缓存失效。缓存不落盘，首次使用不信任持久化标记。

识别按归一化宽度排序，最多四行一批，结果按原坐标顺序还原。真实模型验证发现混合宽度填充会改变正文，因此只合并相同归一化宽度的行。多行概率输出预算为 8 MiB；长行继续单行，避免大字典张量抬高峰值内存。输入缓冲复用，检测输出、裁切和识别输出及时释放，不缓存截图或正文。

识别保持后台串行，每模型两个推理线程，批次与候选之间检查取消。模型按需加载；默认路径不加载韩文，关闭韩文自动识别后在当前任务结束时释放韩文会话，基础会话继续复用。最后任务结束后空闲两分钟释放全部会话及校验缓存，运行库保留既有进程生命周期。

## OCR Provider 详情呈现

本地 OCR 的详情视图独立为 `ocr-provider.slint`，复用 `settings-controls.slint` 中的设置卡片、语言选择器和按钮。页面分为 Provider 标题、识别设置、本地资源；资源状态使用徽标，版本与大小使用对齐信息行，模型名称与大小由 UI bridge 的结构化行模型传入。

主页面显示 MiB，下载确认保留精确字节数。资源就绪时隐藏下载操作；未安装或损坏显示下载或重试，下载中显示进度及取消。删除使用低强调 SVG 文字按钮，并保留原有二次确认和资源生命周期规则。错误在资源卡片内换行显示，不产生新的网络请求或修改下载地址。

## 职责整理（2026-10-11）

Core 的识别策略保留自动、自动加入韩文和指定语言，不携带具体模型名称或固定资源版本。Platform 的模型路由选择对应识别器，资源管理以清单验证版本及兼容性；配置和识别结果不变。

UI 的 OCR 框选宿主与 Provider 设置桥接拆为 `ocr/selection` 和 `ocr/provider`。设置接线不经过框选 Registry；后台识别、资源管理和翻译仍走既有 Core／App 路径。验证见 [职责整理验证](../releases/responsibility-refactor-validation.md)。
