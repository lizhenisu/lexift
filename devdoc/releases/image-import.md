# 文件／剪贴板图片导入验证

日期：2026-10-23。基线 `main` 的 `3ae5124`，实施分支 `codex/image-import`。本记录区分纯回归、组件渲染和原生桌面操作；证据位于忽略的 `target/image-import/`，没有暂存、提交、合并或推送。

## 自动化

| 检查 | 结果与证据 |
| --- | --- |
| Core 全量 | 225 通过，`target/image-import-core-final.log`；补充的导入专项 6 通过，`target/image-import-core-focused.log` |
| App 全量 | 75 通过、1 忽略，`target/image-import-app-final.log`；新增后台导入→编辑器→复制集成检查，打开本身不记历史，复制才记录，像素来自导入底图而非桌面 |
| UI 全量 | 490 通过、11 忽略，`target/image-import-ui-final.log`；包括独立按钮／箭头命中、禁用、桌面绘制模式退出、准备失败工具恢复及文档保留 |
| Windows 编码／读取 | 保存与 WIC 导入 10 通过、2 默认忽略；剪贴板读取 5 通过、2 默认忽略，`target/image-import-clipboard-final.log`；真实 DIBV5 往返与旧式 DIB 系统转换分别手动通过，`target/image-import-native-clipboard.log`、`target/image-import-native-legacy.log` |
| Clippy | Core／Platform／UI／App 全 targets，`-D warnings` 通过，`target/image-import-clippy-final.log` |
| Slint／格式 | UI 编译随回归与 Release 构建通过；`target/image-import-fmt.log`，另检查 `git diff --check` |

覆盖请求去重、取消、过期读取结果、历史／导入宿主编号隔离、无图与错误结果、成功贴图才记历史及 R 记录保留。文件每次导入的来源不同，打开和取消不生成历史条目。App 集成不调用桌面捕获。

WIC 用已知 PNG 验证预乘透明像素，用实际 JPEG EXIF 元数据验证旋转后尺寸；检查损坏、截断及超限尺寸。DIB 纯解析覆盖行填充、方向、调色板、显式 Alpha 和掩码冲突。编码输入使用有界读取，防止文件在读取过程中增长突破上限。

## 视觉

`render/` 中保留 144 张组件图：十二语言、浅深主题、100%／125% 的展开／收起工具条 96 张，加导入菜单 48 张。实际查看了中文工具条、中文菜单及德语深色菜单，标签完整。菜单按文字测量增宽，按钮和箭头不串触发；延迟提示沿用既有 AnnotationHintTarget。

## 原生验证方法与修复

真实输入、原生文件对话框及系统剪贴板使用生成的 PNG／JPEG 和长图夹具；文件名包含中文。脚本按 HWND 检查目标，操作已有窗口，不直接注入 Core 事件来代替按钮。托盘测试通过所属原生回调窗口唤起实际菜单，再使用菜单键盘导航。

早期脚本将文件对话框外框可见误判为控件已就绪；现等待文件名控件，并兼容打开／保存对话框不同的控件 ID 和本地化标题。工作区和 DPI 检查重新读取当前 HWND 的实际几何；早期窗口快照不用于断言最终 DPI。

实测发现，桌面仍选中绘制工具时，直接导入的贴图继承了交互屏蔽，无法打开右键菜单。修复为准备贴图时收尾桌面事务并进入鼠标模式，保留原对象及历史；原生准备失败恢复原工具。新增 UI 回归覆盖成功与失败两条分支，不能仅以贴图可见判定可用。

独立像素复核还发现，当前 Windows 将生成夹具的旧式 `CF_DIB` 转为 `CF_DIBV5` 时保留了额外 12 字节 RGB 掩码；仅按 124 字节头定位会整体偏移三个像素。`clipboard-layout.json`、`clip-8.bin`、`clip-17.bin` 与 `before-fix-pin-copy.png` 保留此轮证据。兼容分支仅在声明像素大小、总块大小、无 profile 以及重复掩码全部一致时调整偏移；直接 V5 和有效 Alpha 仍使用标准布局。纯解析及真实剪贴板转换回归均通过。规范字段参考 [BITMAPV5HEADER](https://learn.microsoft.com/en-us/windows/win32/api/wingdi/ns-wingdi-bitmapv5header)，此处对具体系统转换的描述来自本机证据。

文件编辑器的资源安装和取消使用同一身份锁协调，UI 调用在锁外执行，避免取消清理之后后台再安装无人拥有的底图缓存。

取消文件对话框的原生窗口消失早于后台取消完成。脚本在发起下一条独立请求前留出收尾时间；这段等待仅用于测试串行操作，不计为截图或导入性能，也未加入产品实现。此前过早点击箭头时请求仍被忙碌规则拒绝，不能据此断言菜单失效。

## 最终 Release 实机复测

构建日志：`target/image-import-release-delivery-next.log`。本轮最终 `target/release/lexift.exe` 的 SHA-256 为：

```text
9D4A244A08F5BD4634737DAA3F295A4F37617F9BC4E0A8BD74D1E144DC6F6E6B
```

使用同一二进制，在物理范围 `(0,0)-(2560,1440)` 的 125% 屏和 `(2560,0)-(4480,1080)` 的 100% 屏依次操作。脚本通过真实鼠标、键盘、原生对话框及系统剪贴板验证；没有以直接调用领域事件替代入口。每个脚本结束销毁自己启动的进程。

| 实机检查 | 结果与证据 |
| --- | --- |
| 双屏文件及剪贴板入口 | 中文路径 PNG 打开、原始尺寸保存、取消打开、剪贴板贴图及再次复制均通过；读取实际 HWND、DPI 和辅助窗口样式，导入不改写剪贴板。`target/image-import-native-final.log` |
| JPEG、长图及托盘 | JPEG 文件与 320×2200 PNG 打开、平移、滚轮缩放、Ctrl＋0、保存，以及托盘打开文件通过。历史显示两条“导入图片”；仅打开后关闭没有增加条目。`target/image-import-variants-final.log` |
| JPEG 保存 | 两种 DPI 下切换原生格式并保存，通过文件签名、320×160 原始尺寸和已知颜色检查。`target/image-import-jpeg-final.log` |
| 贴图 OCR | 从剪贴板导入已知文字，通过实际右键菜单识别；实际剪贴板去除空白后精确等于 `IMPORTTEST123`，识别后贴图保留。`target/image-import-ocr-final.log` |
| 独立像素复核 | Pillow 独立解码八份本轮输出，PNG 与夹具逐像素一致，JPEG 夹具误差不超过 3；本次八份最大 RGB 误差均为 0。所有输出修改时间晚于最终二进制，结果与二进制摘要记录在 `target/image-import/verification.json` |

实际窗口截图位于 `target/image-import/native/`，包括两屏编辑器、原生对话框、导入菜单、贴图与图片历史；已查看最终 `editor-120.png` 和 `import-history.png`，小图保持物理 100% 居中，长图历史预览完整适应。自动化夹具和输出属于生成数据，不纳入版本控制。脚本会改变系统剪贴板内容及测试工具条位置，不读取个人文件或现有剪贴板历史。

## 未覆盖范围

未逐项原生实测全部语言／主题、所有剪贴板位深及格式提供者、显示器热插拔或物理负坐标布局；未进行内存不足、磁盘读错误等系统故障注入。DIB 读取首版支持常见 Windows 40／52／56／108／124 字节头及未压缩／位掩码编码，不承诺 RLE 或旧式 BITMAPCOREHEADER。没有增加 HDR 或 ICC 色彩管理。

导入后的所有标注效果、裁剪与 OCR 语言组合未穷尽；相关既有 UI／App 回归通过不等于本轮全部原生组合实测。后台延迟后用户切到外部应用的自动激活保护有身份校验，未声称完成所有系统前台竞争的实机矩阵。历史缓存预算不等于进程总内存上限。
