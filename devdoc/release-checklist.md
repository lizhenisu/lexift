# Windows V0.1 Release Checklist

## 自动检查

- [x] `cargo fmt --all -- --check`
- [x] `cargo clippy --workspace --all-targets -- -D warnings`
- [x] `cargo clippy --workspace --all-targets --features m1-demo -- -D warnings`
- [x] `cargo test --workspace`
- [x] `cargo test --workspace --features m1-demo`
- [x] `cargo build --release --bin lexift`
- [x] `scripts/verify-release.ps1`
- [x] NSIS 安装器、portable ZIP 与 `SHA256SUMS.txt` 生成成功
- [x] 产物内容不含 `.env`、API Key 或个人配置

## Windows 桌面验收

> 自动化清理验收可使用 `uninstall.exe /S /DELETEUSERDATA`。省略该显式参数时，静默卸载与升级卸载始终保留用户数据；交互卸载仍以“删除用户数据”复选框为准。

- [x] 安装器以当前用户安装，无需管理员权限
- [x] 开始菜单项、卸载入口、应用图标和版本信息正确（安装版路径与快捷方式、PE 元数据、EXE/Setup 图标资源已验证）
- [x] 首次启动打开主窗口；`--background` 只显示托盘，不主动显示主窗口
- [ ] 关闭主窗口后应用继续驻留托盘
- [x] 第二次启动不创建第二个实例，并唤起已有实例主窗口
- [x] 开机启动开关立即写入/移除当前用户启动项，重启后保持（注册、移除与持久配置已验证；真实登录启动仍列入最终人工 smoke test）
- [x] 安装目录变化后启动项自动修复为新路径
- [x] 卸载始终删除启动项、产品/卸载注册表项和当前安装路径对应的托盘历史
- [x] 勾选“删除用户数据”会删除 Roaming 配置、Local 日志和 Windows Credential Manager 中的 DeepL API Key
- [x] 不勾选“删除用户数据”时配置、日志和 DeepL API Key 保留，覆盖安装/升级不会丢失数据
- [ ] Chrome、Edge、VS Code、Notepad、Word 与 PDF Reader 划词翻译正常
- [x] DeepL 成功、失败、超时与无凭证状态正确（真实 API 成功/无效凭证测试及错误映射测试通过）
- [x] Translation Popup 不抢焦点；Settings 下拉菜单交互稳定
- [x] Settings 自动保存、Credential 操作和多 Toast 无闪烁回归
- [x] `%LOCALAPPDATA%\Lexift\logs` 最多保留 5 个日志文件，且无敏感内容
- [x] portable ZIP 可从任意目录和不同工作目录直接运行，且不在 portable 目录写配置
- [x] 安装器升级覆盖 V0.1 同版本路径，不损坏设置与凭证

## 发布门槛

- [x] M4.5 桌面人工验收完成
- [ ] 上述 Windows 桌面验收全部完成
- [x] N/A — 当前未配置 Windows 代码签名证书，RC 与 v0.1.0 明确作为 unsigned 发布
- [ ] 创建 `v0.1.0` Tag
- [ ] GitHub Release 含安装器、portable ZIP、SHA256 和发布说明
