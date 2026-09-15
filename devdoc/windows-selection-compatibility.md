# Windows 选择兼容性

本矩阵记录了 Windows 选择流水线中观察到的桌面行为。空白或“Pending”（待定）结果并非推断出的失败。除非某行另有说明，否则请使用普通完整性的 Lexift 进程运行测试。

## 兼容性矩阵

| 应用程序 | 版本 | 场景 | 选择策略 | 延迟 | 状态 / 失败原因 |
| --- | --- | --- | --- | --- | --- |
| Chrome | 153.0.8010.37 | 网页文本、代码块、输入框文本、PDF 查看器 | 待定 | 待定 | 用户实测正常划词通过；逐场景策略和延迟待补 |
| Edge | 153.0.4234.32 | 网页文本、代码块、输入框文本、PDF 查看器 | 待定 | 待定 | 用户实测正常划词通过；逐场景策略和延迟待补 |
| VS Code | 1.137.0 | 编辑器文本、终端文本 | 待定 | 待定 | 用户实测正常划词通过；逐场景策略和延迟待补 |
| Notepad | Windows 内置应用 | 编辑器文本、空选择 | 待定 | 待定 | 用户实测正常划词通过；逐场景策略和延迟待补 |
| Word | 16.0.20326.20144 | 文档文本 | 待定 | 待定 | 用户实测正常划词通过；逐场景策略和延迟待补 |
| PDF Reader | — | 文档文本 | 待定 | 待定 | 当前环境中未找到受支持的独立阅读器 |
| Telegram | — | 消息文本、输入框文本 | 待定 | 待定 | 当前环境中未找到 |
| Discord | — | 消息文本、输入框文本 | 待定 | 待定 | 当前环境中未找到 |
| 提升权限的应用程序 | — | 跨 UIPI 边界选择文本 | 剪贴板不可用 | 待定 | 已知边界：普通完整性的 `SendInput` 可能被阻止；不会自动提升权限 |

## 记录流程

## 2026-09-15 用户本机复测

用户确认 Chrome、Edge、Notepad、VS Code、Word 和浏览器内 PDF 的正常划词翻译可用；
Chrome 网页正文、代码块、输入框、PDF 均正常，结果与选区一致，Loading 和真实译文正常，
UI 保持响应。输入框可在结果出现后继续输入，关闭 Popup 再触发也没有抢焦点。
以上为用户实测报告；具体 UIA/Clipboard 策略和各阶段延迟尚未提供，表中待定项仅表示这些
数值或场景仍缺少记录，不代表正常翻译未测试。

- 强制 Clipboard 文本、空剪贴板、文件测试均为 `1 passed`，分别确认 sentinel 恢复、
  format count 为 0、原文件格式保留。
- 图片测试准备有效位图后重测通过（2026-09-15，`1 passed`）：检测到 bitmap/image，
  复制后 sequence 变化且读取到非空 Unicode 选区，恢复后 `original formats preserved=true`。
  10.08 秒的测试总时长包含 10 秒人工切换倒计时，不作为捕获延迟。
  此结果确认格式保留；实际图片粘贴、内容和尺寸核对沿用 M3.3.1 已通过的记录。
- 断网错误及恢复网络后重试正常；管理员应用返回 No selection，未卡死。
- 无选区快速松开两键时正常显示 No selection、不弹错误窗口。长按 Alt 约一秒的修复后
  复测也通过：03:10:30、03:10:33 两次请求均在 UIA 无选区后，因按键仍按住而安全跳过
  Clipboard 复制，随后收到 `selection_capture_empty`，进入 `NoSelection`，
  `task_id=None`、`command_count=0`。用户确认不再出现按键超时错误。
  两次总捕获耗时为 198ms、179ms，其中 Clipboard 等待为 170ms、166ms；该路径不执行复制。
- A → B 重叠翻译复测通过：03:04:55.895 任务 1 开始翻译，03:04:57.007 任务 2
  开始翻译；03:04:57.330 任务 1 返回时仍保持任务 2 的 Translating 状态，随后任务 2
  于 03:04:57.939 完成，用户确认最终显示 B。两次捕获均使用 UIA，耗时分别为 18ms、4ms。
  此后任务 3、4 也分别完成，两次 UIA 捕获耗时为 6ms、5ms。样本不作为总体性能统计。
- 单独按 Alt 再松开会切换浏览器菜单焦点，该操作不等于两次完整 Alt+X，连续快捷键场景
  两次完整组合键的 A → B 重叠翻译已按上述日志确认通过；捕获期间重复请求由自动化测试覆盖。

M3.4 核心验收通过，可以进入 M4；独立 PDF Reader、Telegram、Discord 和逐应用策略、延迟记录仍待补充，不宣称全矩阵覆盖。

对于每个场景：

1. 按测试夹具要求，将已知文本、图像或复制的文件放入剪贴板。
2. 在目标应用程序中选择一个不同的源字符串，并按一次 `Alt+X`。
3. 确认弹出窗口仅在捕获后出现，不会将焦点留在 Lexift 中，并显示所选源文本及其后的 DeepL 结果。
4. 粘贴原始剪贴板夹具，并验证其内容和类型均得到保留。
5. 从跟踪记录中记录 `strategy`、`uia_latency_ms`、`clipboard_latency_ms` 和 `total_capture_ms`。
   切勿将源文本复制到此文档或日志中。
6. 在没有选择的情况下，以及快速按两次 `Alt+X` 的情况下重复测试。无选择必须进入 NoSelection 且不弹出窗口；进行中的捕获必须忽略重复请求。

## 手动剪贴板测试

被忽略的 Windows 测试覆盖文本、空、位图/图像、文件以及竞争性剪贴板更新：

```powershell
cargo test -p lexift-platform interactive_selection_capture_restores_text_clipboard -- --ignored --nocapture
cargo test -p lexift-platform interactive_selection_capture_restores_empty_clipboard -- --ignored --nocapture
cargo test -p lexift-platform interactive_selection_capture_restores_image_clipboard -- --ignored --nocapture
cargo test -p lexift-platform interactive_selection_capture_restores_file_clipboard -- --ignored --nocapture
cargo test -p lexift-platform interactive_newer_clipboard_content_wins -- --ignored --nocapture
```

这些剪贴板夹具的 M3.3.1 验收结果记录在
`devdoc/implementation-plan.md` 中。未实测的应用和场景继续标记待定；已通过的用户实测与自动化结果分别记录。
