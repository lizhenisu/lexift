# Lexift 0.0.1-rc.8

本候选版本改进翻译服务配置，并统一 Windows 窗口在任务栏和 Alt+Tab 中的显示规则。

## 更新内容

- 新增 Ollama、LM Studio 和 DeepSeek 翻译服务配置与模型管理，并重构翻译模型选择和参数设置。
- 改进长译文布局、Provider 设置交互、模型检测及窗口恢复体验。
- Windows 上只有 Settings 窗口出现在任务栏和 Alt+Tab；主窗口、翻译 Popup、标注工具窗口及原生保存对话框均保持排除，同时保留各自的输入与激活行为。
- 修复划词工具条遮挡选中文字的问题，并回归图片剪贴板启动后原应用 Ctrl+C／Ctrl+X 快捷键。

## 安装

下载 `Lexift-0.0.1-rc.8-windows-x86_64-setup.exe` 安装，或下载 portable ZIP 解压运行 `lexift.exe`。

这是候选版本。请通过 GitHub Issues 报告问题。
