# Windows 多区域截图验证

基线为 `bf2ffef`，实现分支为 `codex/screenshot-multi-region`。

- 冻结桌面一次，按创建顺序后台裁切，并复用快速保存的 PNG 原子发布。每张成功发布的图片使用独立历史来源。重新选择目录时保留已成功项和原编号。
- Core 夹具覆盖重叠命中、删除重排、负坐标和跨屏像素、显示器间隙、部分失败重试、过期回调和 R 的独立性。
- 工作区首轮库回归、追加的 Core 多区域及图片历史测试、格式、严格 Clippy 和 Windows Release 构建通过。Platform 既有回归验证 PNG 并发发布时不会覆盖同名文件。
- 最终 Release 在 100% 和 125% 屏分别从真实菜单进入，连续创建区域、按 Delete 重排、取消目录选择后再次保存。100% 屏同时验证鼠标保存按钮、移动及边缘调整。原窗口在冻结后变色，两张 PNG 仍逐像素等于冻结时的夹具颜色。
- Core 验证部分失败重试。原生部分写入失败后的重试、跨屏拖动、全部语言和主题的视觉矩阵未实测。
- 原生输入脚本、初次失败记录、输出 PNG、窗口截图和最终构建哈希位于 `target/screenshot-multi-region/`。最终通过记录见 `native.log`、`native-final-96.log` 和 `native-final-adjust-02.log`。
