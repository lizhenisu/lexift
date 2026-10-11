# OCR 自动检测验证

日期：2026-10-11。未暂存、提交或上传资源。已有 OCR Provider 改动保留。

## 实现

- 设置增加自动检测；新配置默认 `auto`，旧文件缺失字段保留跟随界面，人工代码保留。沿用 Schema 10；旧文件反序列化与无文件默认路径分别测试。
- Core 冻结识别策略；Platform 共用检测和预处理，自动调用 v6 small 与韩文 v5 mobile，人工只调用对应模型。正文概率、空间位置保留用于按连续韩文范围选择，避免逐字符 CTC 位置差异导致丢字；不抢占图像两端，不覆盖其他文字。
- 校准仅使用 51 张图片，57 张不同文本图片用于验收。字符级编辑距离对齐标签，概率分箱、Beta 平滑、单调回归；选择门限在校准集上搜索，平局优先保守参数。校准绑定资源 ZIP SHA-256。
- 生成夹具采用微软雅黑与 Malgun 字体回退，避免把缺字方框错误标注为原文字。所有生成图片、正文及候选留在忽略目录，日志仅包含统计和测试结果。

## 资源

v2 ZIP：36,907,916 bytes（约 35.2 MiB）；SHA-256：`6520f723fd3ebcb80bb98cda8a5037d9b6c8e07b379a923c6c1511926bbdc6cd`。

固定官方模型修订、输入形状、BGR 预处理、CTC 字典类别数及输出概率分布均已核对。Windows 导出环境见 `scripts/ocr-build-requirements.txt`。复用 v1 的 ONNX Runtime 1.22.0 DLL，字节相同，无进程内 DLL 升级。

完整 ZIP 已经本地 HTTP 下载、逐文件校验和原子启用测试；DLL 已加载后的延迟删除与下一独立进程启动清理分别通过。兼容旧 v1 待删除标记，禁止未知目录删除。生产 Release 尚未发布，生产 URL 不作为已验收下载地址。

本机将已逐文件校验的 v2 放入 `%LOCALAPPDATA%/Lexift/ocr/v2` 用于验证，v1 与用户配置保留。

## 真实模型与性能

108 张生成图片通过真实 ONNX CPU 推理，覆盖十二种语言代码、三种字号／清晰度、数字标点、中英／日英／韩英／中韩混合及无空格中韩连续行。它们是合成打印文字夹具，不代表真实桌面、手写或任意字体的全面准确率。

独立进程小选区 CPU 测量：自动冷启动 326ms、自动热启动均值 73.15ms、v2 人工均值 49.82ms、v1 人工均值 47.42ms。模型缓存后的自动成本约为旧人工的 1.54 倍。该进程峰值工作集约 110.5 MiB，PrivateBytes 约 123.2 MiB；数据为测试进程，不是完整 App 内存，不能外推到大图。

最终字符错误率与回归结果见本记录的交付结果；候选记录和逐项统计留在 `target/ocr-auto-corpus`，不提交正文。

## 复现

```powershell
python -m pip install -r scripts/ocr-build-requirements.txt
python scripts/build-ocr-resources.py --runtime-dir build-keep/ocr-resources/files
python scripts/validate-ocr-auto.py generate
$env:LEXIFT_OCR_TEST_RESOURCES = (Resolve-Path target/ocr-resources-v2/files).Path
$env:LEXIFT_OCR_CORPUS = (Resolve-Path target/ocr-auto-corpus).Path
cargo test -p lexift-platform real_automatic_corpus --lib -- --ignored
python scripts/validate-ocr-auto.py calibrate
cargo test -p lexift-platform real_automatic_corpus --lib -- --ignored
python scripts/validate-ocr-auto.py report
```

`calibrate` 会更新源码中的聚合参数，仅供开发显式调用；最终验收不运行校准，避免修改已固定参数。实际发布资源需另行确认。

## 交付结果

- 相关 crate 回归：App 54、Config 23、Core 133、Platform 184、UI 367，共 761 项通过。默认跳过的 100 项包含其他原生/GPU/资源专项，不视为执行通过。
- OCR 专项另外执行：真实十二语言模型、108 图 corpus、性能、本地 HTTP 完整 ZIP、真实 DLL 占用及下一进程清理；均使用真实 ONNX Runtime。测试要求不同进程的删除/清理已分别执行，不能用并行运行替代。
- 独立验收集 57 图：整条识别流程字符错误率 0.73%，直接行候选为 0.18%；混合文字子集约 4.76%，仍有空格与个别字符误差。无空格中韩连续样本通过，非韩文样本无误选韩文。此数据限定于当前合成打印夹具，未声称任意混合文字均已验收。
- 12 种语言 × 浅深色 × 100%/125% 共 48 张软件渲染截图检查完成；实际 Release 在双屏 125%（DPI 120）与 100%（DPI 96）确认了识别语言列表、即时保存及资源 v2 就绪。
- 从新 Release 通过 Alt+T 创建双屏遮罩，框选设置页文字，Popup 显示 OCR 原文并成功生成英文译文。框内下拉箭头被识别成字母属于该样本的图形误识别边界。测试后通过 UI 恢复原先的“跟随界面语言”，并检查文件中 `ocr_language` 缺失，未覆盖其他设置。
- 格式、差异空白检查、相关 crate Clippy（`-D warnings`）、Slint 编译和 Windows Release 构建通过。
- Release：`target/release/lexift.exe`；SHA-256：`102BA2CD7FE593D7A8D25BFF77D1A38050CC9BC9C04DD283D62726B5974CE63D`。
- 实机与软件渲染截图在忽略的 `target/ocr-auto-screenshots`；统计、性能和正文结果在 `target/ocr-auto-corpus`，正文不进入版本控制或日志。

## 未覆盖范围

生产资源 URL、安装包/便携包更新、全面真实桌面准确率、手写/特殊字体、复杂排版、大图性能及首帧逐帧录屏未在本轮完整验收。未重新执行实际任务栏/Alt+Tab 列表、跨屏拖动和焦点的全部原生回归；本轮没有修改屏幕宿主与窗口策略。以上不以最终截图或模型单元测试代替。

## 2026-10-11 本机下载失败复核

- 公共 `ocr-v2` Release 资源尚未发布，下载 URL 返回 404；本机测试可用不代表生产下载已验收。
- 用户反馈页面显示未安装及本地占用 0.0 MiB。复核本机 v2 全部文件的长度与 SHA-256 通过；独立 debug/Release 库检查均返回 Ready。旧运行实例中的状态差异未取得足以确定根因的证据，不能仅归因于安装版抢占。
- 增加低频资源目录和检查结果诊断，不记录截图或 OCR 正文。重新构建并从 `target/release/lexift.exe` 启动后，实际进程日志返回 Ready、54669705 bytes；实际 Settings 显示资源就绪及 52.1 MiB，并观察到框选结果进入 Popup 开始翻译。
- 当前 Release SHA-256：`3DF2A3191D26F67A7B8CD63B5E2BECF186E212932D10B68E8AFF14FF2F48E24C`。平台 Clippy、格式及 Release 构建通过，使用本机安装目录执行真实十二语言推理回归通过。
- 本次没有发布资源、改动下载地址、替换模型或修改用户配置；仍需单独授权并发布资源后验证公共下载。
