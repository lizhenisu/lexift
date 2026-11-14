# Windows 截图选区加减验证

基线为 `c427d75`。脚本、日志和输出位于 `target/screenshot-selection-combine/`。

- 工作区库回归、严格 Clippy、Slint 编译、格式及 Windows Release 构建通过。Core 覆盖重叠加选、孔洞、补回、清空、撤销、负坐标、跨屏间隙、预算，以及失败重试、旧结果、图片历史和 R。
- 最终 Release 在 100%／125% 屏各跑一轮单屏操作，另跑一轮跨屏拖动。验证模式切换、Ctrl+Z、取消另存后重试。冻结后移动源窗口，独立解码确认单屏颜色与冻结参考一致至每通道 1，孔洞 RGBA 全零。跨屏检查 Alpha。证据见 native-96.log、native-120.log、native-cross.log 及 final.sha256。
- 多区域、多窗口、多边形原生回归通过。多窗口夹具最初未确认前窗处于遮挡位置，保留失败日志。固定夹具层级并核对鼠标命中后重测通过，见 regression-window-ordered.log。
- 普通截图纳入自动化回归。外部前台切换、窄工作区、全部语言及浅深主题的完整视觉矩阵未单独实测。
