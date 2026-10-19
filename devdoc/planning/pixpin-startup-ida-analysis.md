# PixPin 截图启动：DLL 调用链分析

日期：2026-10-19。对象为本机 PixPin 3.5.5.1 的发行文件；通过 IDA Free 9.4 与 Native MCP 分析副本。本文是静态反编译证据，不代表已跟踪本机每次截图实际走过的分支，也不能为各项机制分配独立的耗时收益。

## 1. 文件身份与证据

| 文件 | SHA256 |
| --- | --- |
| UiRegionDetector.dll | `e8b65cc6008f414d6eb9abaa7d766779f1b99991f34e108c491613a87baa6f1f` |
| PixScreenManager.dll | `986e0bd6329d3cb062ddca8ff8e85a216306947d5c04da5f2bbaf619dd8bd23b` |
| PixWinCapture.dll | `8cdd96d2327cc2ebe553ee242667834ff9da1f90c667ce978dcedb2adca97da5` |
| PixWin32CaptureCore.dll | `b7ccd8a43da1005010191ede16f6995ca22c09cb4d4332d0c3074ec97232e85b` |

原始反编译、目标摘要与 REA Evidence ID 保存于忽略的 `target/pixpin-analysis/startup-dll-evidence-20261019.json`。主程序的先前证据见同目录 `startup-followup-20261019.json`、`screenshot-startup-evidence-20261018.json`。本次还检查了 UiSpy.dll 导出，发现相近实现，但没有建立它属于当前截图入口的调用关系；不把它作为活跃路径的依据。

反编译变量名、类型及函数名不等同于原始源码。尤其 `WinGetWinRectByPointUIA` 的名称不能证明使用 Microsoft UI Automation；本次沿虚表确认的实现实际包含 MSAA 与 HWND 查询。

## 2. 内部区域识别

### 顶层窗口快照，命中后才创建内部查询对象

- `UiRegionDetector.dll!0x180003F90`：构造探测器并调用 `SnapshotAllWinRect`。
- `0x180005990`、枚举回调 `0x180004EB0`：调用 `EnumDesktopWindows`，读取可见性、排除 HWND、穿透样式、DWM 隐藏状态与扩展边界，构建窗口列表。这里未遍历所有窗口的内部可访问性树。
- `0x180006500`：先找包含鼠标点的窗口，再按窗口策略创建所需查询对象。`0x180003890` 使用 `AccessibleObjectFromWindow`；另一路 `0x1800032A0`／`0x1800030D0` 构造 HWND 查询对象。
- `0x180003960`：MSAA 路径通过 IAccessible 虚表的 `accHitTest` 逐步命中，循环上限约 31 次，不是先穷举整棵树。
- `0x180003320`：HWND 路径调用 `ScreenToClient`、`ChildWindowFromPoint`。

证据：`ev_1b02e2a444cf8ca05c90c33ea522fdd76707caee496e119cac3d3b7f4070e735`、`ev_5c3e77803f762b07db6416277db6d8401658e4528bde6b5d44b9f9c815b7d3f2`、`ev_574002bc12f48f79ddceb79f2980c7ed3eaeb3aeb7f620f8607a6b92a7902e64`、`ev_e4a0b0868b623cdb2ff41f0e42c170e61ae6cf8e55534c2b8640d03debe3bf0a`、`ev_ea21d0c71de50d966f2e35044082320ea6c8b8fc2fcd035afb5c138f30e25ff5`、`ev_1967d14001de5b06b677d8913b6e62b79acf19c05a4eb769fa989299a843119f`。

### 后台查询合并与区域缓存

- `0x180001360`：启动 QThread 并立即提交初始鼠标位置查询。
- `0x1800022C0`：提交请求前删除队尾连续同类型的待处理请求，再写入最新请求；不同类型之间保留顺序。不是无条件清空全部队列，也不能据此宣称正在执行的阻塞请求可被打断。
- `0x180002810`：线程逐项处理位置、父级、子级等请求，携带请求序号及排队／执行／总时间，通过结果信号交给 UI；循环含 `msleep(1)`。
- `0x180005F70`、`0x180006200`：滚轮选择父级后设置缓存标志和矩形；鼠标仍在矩形内时可直接返回。此缓存不能推广为所有悬停元素均缓存或跨截图缓存。
- `0x180006200`：首次类型查询耗时超过 `0x12C`（300ms）会把该窗口类型改为另一查询策略。它是调用返回后的降级判断，不是能中断调用的 300ms 超时。
- `0x180006A50`：后续查询会检查 HWND 及当前 DWM 边界，不能称为完全不查询系统或完全冻结的元素列表。

证据：`ev_fa2a8d4cadcaabbeffcdc7236b75b1a54583bf818cdcf28d3f861ad6e53e7e7e`、上述 `ev_1b02…`、`ev_5c3e…`。

主程序 `0x1402D69B0` 仍有 `getFirstRect` 等待：未就绪时 `msleep(1) + processEvents`，并校验对象／探测器身份；取得首个结果后接入后续结果信号。不能说 PixPin 完全不等待，也不能说它先收齐所有内部区域。

### 浏览器的局部兼容处理

`0x1800067C0` 对匹配的前台进程调用 `AccessibleObjectFromWindow` 及一个 IAccessible 属性读取，日志明确提到 Chrome；按窗口身份记录处理，表项超过 32 时清理。可确认存在一次性的兼容触发及去重。它是否开启特定浏览器可访问性模式、能节省多少时间，本次没有运行证明。

## 3. 抓屏与资源复用

### 管理层有多后端，但“DXGI”日志不等同于 Desktop Duplication

- `PixScreenManager.dll!0x180003B40`：按系统版本及引擎配置选择屏幕实现；日志称 DXGI，失败回退 Qt。
- `0x180006E70`、`0x180006AB0`：建立 `PixWinCaptureStatic`，取得预期图像尺寸后采集至 QImage；尺寸变化时重建目标图像并重试。
- `0x180003450`：范围被单屏包含时直接调用该屏采集；只有直接结果为空时才创建目标 QPixmap，并以各屏交集绘制合成。主程序背景层也按屏调用，不必始终先拼接整个桌面。

证据：`ev_f363cffa311ba4cd7dcddea7405f6e4037fd776a504ae44c7a16bd504e97f202`、`ev_be3dd28e8b19fabaac661d89a6053600ed8985f5e8d79a889d418f6d345e5385`。

### 已追通的静态采集路径实际使用 WGC + D3D11

- `PixWinCapture.dll!0x180004760` 创建 `PixWin32CaptureCoreStatic`。
- Core DLL `0x18000C270` 创建 D3D 设备及上下文；`0x18000D520` 准备 capture item、frame pool 和 session，frame pool 参数为 2 帧。
- `0x18000D840` 调用 WGC session 启动及 frame pool 取帧。生成代码路径明确为 `winrt/Windows.Graphics.Capture.h`；它会排空已有帧，再等待新帧，循环 `Sleep(1)`，最长 3000ms。3000ms 是失败等待上限，不是固定启动延迟。
- `0x18000DE90` 将 staging texture Map 到 CPU，按 RowPitch／目标 stride 逐行 memcpy，再 Unmap。因此该路径不是图像零拷贝。
- 本次没有建立 Desktop Duplication 的活跃调用链，也没有证明每次本机基准都采用此 WGC 分支。

证据：`ev_914028f3d58580395c805879913e91fbf495e909bfcd377dc5e384f6bdc128d6`、`ev_8996dee50119c24c91b8c385eb5a75df4319a11c67ba1c7fa5d2ffb6a2a457ed`、`ev_cd91253fb07b16d03655a7214e316a49bb94ea24ebbdd88ef5524bfe152dac4e`。

### 捕获资源有短期复用与空闲释放

- `PixWinCapture.dll!0x180004760` 配置 3000ms 单次 QTimer；`0x180005200` 每次采集后重新启动计时。
- `0x1800061E0` 到期调用 `releaseCapture`；Core `0x18000E440` 关闭 session 与 frame pool，保留对象中的设备等其他成员。
- Core `0x18000D840` 在 session 存在时可继续采集；关闭后才通过 prepare 重建。
- 由此可推导短时间连续采集可复用部分资源；不能推导整个应用永久保留截图窗口，也不能据此量化冷启动收益。管理器 refresh 也有清理路径，并非所有对象始终复用。

## 4. 对 Lexift 的启示与限制

| 差异 | 判断 |
| --- | --- |
| PixPin 顶层快照后按点查询，Lexift 抓图前有 80ms 内部元素采集预算 | 最直接的结构差异。若采用类似策略，可移除该前置工作，但必须重新决定冻结元素一致性及后台查询契约；本次未修改实现 |
| 连续同类请求合并、局部区域缓存 | 能避免鼠标事件积压；不能解决一个已经阻塞的可访问性提供者 |
| 设备、session／frame pool 的局部复用 | 值得对重复 WGC 路径做实验。Lexift 本机正式样本 40/40 已用 GDI 快路径，不应拿此机制解释全部约 120ms 差距 |
| 单屏直接采集、跨屏才拼接 | 已确认减少无必要合成的设计；Lexift 空文档合成当前约 5ms，优先级低于首帧呈现 |
| Lexift 派发至实际反馈约 160ms | 本次没有深入 PixPin 的完整窗口呈现链，也没有测出它对应阶段耗时；不能用 Qt 名称直接认定为 GPU 渲染优势 |

下一步最有价值的是对 Lexift 的原生准备、软件绘制及 DWM 交接分别计时，再做保持一致性要求的识别前置成本实验。若要照 PixPin 改成截图期间按点查询系统，需明确变更先前已确认的“冻结元素、不在鼠标路径查询系统”约束，不能当作等价小优化直接替换。

## 5. 覆盖与未覆盖

本次只做发行文件静态分析与分析工具自动化，未执行新的竞品性能基准、API 调用追踪或 Lexift 代码修改。沿用的实测仍见 [性能记录](../releases/screenshot-startup-performance.md)。未确认完整缓存策略、所有引擎分支、呈现后端、每项机制独立耗时及完整故障覆盖。本文细化并取代此前“实际抓屏 API 尚未确认”的笼统表述，不追溯修改先前实测数字。
