# Slint 1.19 升级验证

## 变更

- 分支：`codex/slint-1-19`，基线：`main d855ee2`。
- Slint、slint-build、i-slint-core 和 i-slint-backend-winit 统一升级至 1.19.0。
- image 固定版本同步至 0.25.10，以满足 Slint 1.19 的版本约束；没有新增 crate 或依赖入口。
- 正式 UI 显式保留 Slint 1.18 语言 edition，保持既有条件布局与状态行为；测试用内联组件显式选择 1.19。Rust edition 不变。
- Slint 1.19 调试编译器在标注参数结构体成员的双向绑定处触发 binding_analysis 断言。改用属性读取与显式回写，复用原参数事务及历史接线；CheckBox 用用户切换回调回写。

## 验证

证据目录：忽略的 `target/slint-1-19/`。依赖下载遇到网络中断，已校验下载包与 Cargo.lock 的校验和，后续检查使用 `--offline --locked`。

### 自动化与构建（2026-11-20）

- `cargo test --workspace --offline --locked`：通过。App 89、Config 25、Core 380、Platform 239、Translate 43、UI 558 项通过；原有显式原生/资源测试仍按其忽略约定执行，不冒充本轮实测。
- `cargo clippy --workspace --all-targets --all-features --offline --locked -- -D warnings`：通过。
- `cargo fmt --all -- --check`、`git diff --check`：通过。
- `cargo build --release -p lexift-app --offline --locked`：通过。
- 最终 `lexift.exe` SHA256：`a52a42db6c3334047ced0f64bdb44d1eb4a5b0cb9c1826a46e1b45a006586802`。
- UI 回归包含十二语言、浅深主题、DPI、指南及设置布局、参数事务、画布、贴图及录屏预览；抽查中文浅色与德文深色窄布局截图，无截断。

### 发现与处理

- 保留最初编译断言、主题测试失败日志。主题测试原采样点位于 SVG 图标中，1.19 的图标栅格像素变化导致背景颜色断言失败；实际悬停背景正常。采样移至空白内边距，原悬停、按下、展开颜色断言全部通过。
- 单独测试与全量 UI 测试并行运行时，Windows 链接器曾因正在运行的测试 EXE 被占用而报 LNK1104。等待退出后，最终完整回归正常通过；没有通过修改产品绕过文件锁。
- 1.19 编译器报告既有尺寸输入框外层与内部 TextInput 的重复无障碍语义警告；本轮保留原输入结构。该编译器提示不属于 Rust Clippy 错误，无新增翻译文案。

### Windows 原生验证

脚本：`target/slint-1-19/native.py`，结果：`native.log` 和 `run-final/`；最终 Release 上执行，正常退出。使用合成颜色行号图，无用户图片。

- 100%（96 DPI）与 125%（120 DPI）分别执行静态截图、长截图采集直接输出、长图编辑输出。
- 实际剪贴板与自动保存 PNG 对照参考图，尺寸及像素一致。
- 设置经真实托盘入口打开，原生目录选择覆盖取消、确认及重启后保留；更改目录后生效，目录失效时复制仍成功并按既有保存失败反馈处理；关闭自动保存后不产生文件。
- 测试退出后还原原配置，销毁测试应用。证据的二进制哈希与最终 Release 一致。

未覆盖：本轮未重新执行真实 OCR 模型识别、真实音频录制与 Media Engine 回放完整矩阵，也未实测非 Windows 平台；相关领域及 UI 自动化回归已通过。本轮不改变其业务链路。

## 参考

- [Slint 1.19.0 发布](https://github.com/slint-ui/slint/releases/tag/v1.19.0)
- [语言 edition 行为变化](https://slint.dev/releases/1.19.0/docs/slint/guide/language/concepts/editions/#behavior-changes)
