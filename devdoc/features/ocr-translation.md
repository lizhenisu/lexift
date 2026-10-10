# 本地 OCR 框选翻译

## 使用

- 默认快捷键 **Alt+T**，可在 Settings → 热键设置修改；与划词翻译、标注快捷键互斥。
- 首次使用在 Settings 中确认下载全部资源。下载完成后再次按快捷键。
- 先冻结所有显示器，再显示独立遮罩。拖动框选，松手识别；无面积点击继续等待。Escape、右键或再次按快捷键取消。
- OCR 原文及当前服务生成的译文显示在既有 Popup 中。识别失败或空结果不翻译；翻译失败保留可编辑原文。
- 标注进行中拒绝框选并提示退出标注；框选不使用标注 Registry。
- 识别语言独立于翻译语言，默认跟随界面语言。识别完全本地执行；翻译仍遵循选中服务的联网规则。

## 模块与生命周期

Core 定义 `OcrState`、版本化事件／命令及 `OcrPort`。Platform 提供一次性 GDI 屏幕采集、校验下载及 ONNX CPU 推理；App 调度阻塞工作，UI 管理临时遮罩。冻结图像和正文不写日志或普通配置。

每次请求递增 generation。关闭所属 Popup、取消或替换请求后，旧识别结果不能启动翻译。遮罩按需创建，结束即销毁；所有遮罩遵循辅助窗口策略。推理 Session 在最后一次任务结束后空闲两分钟释放，再次使用按需创建。

屏幕和选区坐标均为物理像素。不同显示器之间的空隙补白。检测图最长边缩至 960，映射回原图裁切识别，DB 连通区域展开、CTC 解码并按阅读顺序保留换行。单次选区最多 4000 万像素，过大选区明确报错。

## 框选窗口基础

原生窗口准备、首次绘制托底及共用手势边界见 [屏幕区域选择基础](region-selection.md)。取消监听与外观准备已分离；首次显示验证见 [首帧记录](../releases/ocr-first-frame-validation.md)。

## 资源构建与发布

客户端不调用 Python。资源位于 `%LOCALAPPDATA%\Lexift\ocr\v1`，安装版与便携版共用。

`scripts/build-ocr-resources.py` 在开发环境使用官方 Paddle2ONNX 导出固定 Hugging Face 官方模型修订，生成模型、字典、ONNX Runtime Windows x64 DLL、许可和第三方声明的 ZIP，以及编译进客户端的 `assets.json`（版本、精确字节数、每文件与 ZIP 的 SHA-256）。输入模型修订记录在清单中。

本次导出环境：Python 3.12、PaddlePaddle 3.2.2、Paddle2ONNX 2.0.1、ONNX Optimizer 0.4.2、ONNX Runtime 1.22.0。Windows 转换器存在 DLL 兼容问题，已在隔离 WSL 环境完成导出，并由 Windows ONNX Runtime 实际识别验证。Windows 打包环境仅使用 ONNX Runtime 1.22.0 的 Windows DLL。

开发端可执行：

```text
python scripts/build-ocr-resources.py --runtime-dir <Windows onnxruntime/capi 目录>
```

实际 ZIP 输出至忽略的 `target/ocr-resources/lexift-ocr-v1-windows-x86_64.zip`，不进入 Git。下载只启用全部验证成功的目录，采用 staging → version 原子重命名。取消会中断网络 Future，损坏或失败不会覆盖已有版本。DLL 仅从校验后的绝对路径加载。

**资源 Release 尚未发布。** 清单中的生产地址是待发布目标，不表示该地址当前可下载；发布资源前须另行获得用户确认。未发布阶段以本地 HTTP 夹具验证下载流程，不能称生产下载已验收。

## 验证记录

本轮记录见 [OCR 实施验证](../releases/ocr-validation.md)。固定识别图在 `crates/lexift-platform/tests/fixtures/ocr/`；真实推理测试需先构建资源：

```powershell
$env:LEXIFT_OCR_TEST_RESOURCES = (Resolve-Path target/ocr-resources/files).Path
cargo test -p lexift-platform real_paddle_models -- --ignored --nocapture
```

该环境变量仅由测试读取，不是客户端资源路径或下载地址覆盖机制。
