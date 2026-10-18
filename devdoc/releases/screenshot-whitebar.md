# 首次框选顶部白条修复验证

日期：2026-10-18。本次在已有未提交的像素放大镜修改上修复原生激活重绘；没有暂存、提交或推送。

## 原因与修复

截图框选宿主已有无边框样式保护、WM_NCCALCSIZE 与 WM_NCPAINT 处理，但 WM_NCACTIVATE 仍按默认参数向后传递。为首次框选的 C 取色激活窗口后，默认非客户区绘制可能覆盖画面顶部，出现原生标题栏残影。

仅在屏幕选区宿主的原生子类过程中，转发 WM_NCACTIVATE 时将 lParam 设为 -1。Winit 继续收到完整的激活／失活状态转换，DefWindowProc 不重绘非客户区。没有吞掉焦点通知，也不改变通用窗口激活、辅助窗口身份或首帧交接。依据：[Windows WM_NCACTIVATE 文档](https://learn.microsoft.com/en-us/windows/win32/winmsg/wm-ncactivate)。

## 自动回归

- 在真实 HWND 上，先安装观察后续消息的原生子类，再准备选区宿主。模拟激活、失活、重新激活，并先执行后端样式重写。修复前记录为 `(1,4660), (0,4660), (1,4660)`，回归失败；修复后为 `(1,-1), (0,-1), (1,-1)`，回归通过。确认消息继续传递，样式与物理客户区不变。
- Platform 屏幕选区回归 3 项通过；`target/screenshot-whitebar-platform.log`。修复前失败证据：`target/screenshot-whitebar-before.log`。
- UI 框选宿主 7 项、截图键盘 8 项通过，包括首帧激活、过期身份、外部前台与 C 重复屏蔽；`target/screenshot-whitebar-ui-selection.log`、`target/screenshot-whitebar-ui-keyboard.log`。
- Platform 全目标 Clippy、格式检查通过；`target/screenshot-whitebar-clippy.log`、`target/screenshot-whitebar-format.log`。

## 最终 Release 原生验证

最终 Windows Release 构建通过（2m10s），日志为 `target/screenshot-whitebar-release.log`。原生测试校验项目 `target/release/lexift.exe` 的进程路径；PID 39760，文件 27,305,472 字节，SHA-256 为 `92741E287B826C315A98081907791C7BAB2F9398D53CFE3709B216CD111FC1CD`。身份记录为 `target/screenshot-whitebar/binary.json` 与 `process.json`。

实机为 2560×1440／125% 与 1920×1080／100% 两屏，使用既有均匀底色与已知色块夹具，检查实际桌面呈现：

- 首次激活、主动失活、点击重新激活 125% 屏、点击重新激活 100% 屏、退出后重开，共 5 个阶段、10 张全屏截图。各屏顶部 64 行与相同底图的稳定行逐像素比较，总计 1,433,600 个像素，差异为 **0**。所有阶段窗口范围与显示器范围一致，无标题栏文字、白条或几何偏移。
- 首次框选两屏 C 复制均为夹具中心 `#777488`，分别核对 135 格原始像素；重新激活、开始框选转入编辑、退出重开后 C 继续正常。外部夹具取得前台时 C 不写剪贴板。
- 读取各阶段 HWND 的样式、扩展样式、位置及前台身份，无原生标题栏／粗边框、无 WS_EX_APPWINDOW，保留 WS_EX_TOOLWINDOW。测试结束关闭本次创建的应用和夹具进程。

证据全部位于忽略的 `target/screenshot-whitebar/`：`native.log`、`result.txt`、`top-pixels.jsonl`、`first-activated-*.png`、`deactivated-*.png`、`reactivated-125-*.png`、`reactivated-100-*.png`、`reopened-*.png`。夹具与原生脚本也仅存于 `target/`。

此前像素放大镜实测核对了鼠标旁网格，但没有专门逐像素检查首次激活后的顶部，因此没有覆盖用户此次报告的残影。之前记录不作为本次问题已通过的证据。

## 未覆盖范围

本次原生测试使用当前 Windows 主题、中文界面及上述两屏布局，没有穷尽全部 Windows 主题、远程桌面、负坐标屏幕或不同显卡驱动。检查为激活后各阶段实际截图，未作逐刷新周期的高速录屏；消息回归覆盖激活链路本身，不依靠延时重绘或持续修补隐藏残影。OCR 共用屏幕宿主，但本次没有另跑 OCR 端到端流程。
