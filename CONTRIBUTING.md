# 贡献指南

欢迎提交 Bug、兼容性反馈和小范围改进。较大的界面或数据源变更请先通过 Issue 说明场景与方案。

## 开发环境

使用 Windows x64、Rust stable MSVC、Visual Studio Build Tools C++ 桌面开发组件和 Windows SDK。应用依赖 Win32 / Direct2D，暂不支持其他系统或 WSL 构建。

提交 `Cargo.lock`，构建使用 `--locked`。分支建议使用 `codex/` 或描述目的的前缀，提交按 `feat:`、`fix:`、`docs:`、`chore:` 等说明变化。

## 自动检查

```powershell
.\scripts\Test-CI.ps1
```

脚本检查格式、严格 Clippy、编译全部测试，串行运行不需要真实账户或 Explorer 操作的合成测试与清单检查。公共 CI 不查询真实额度，不缓存账号、应用数据或构建产物。

完整桌面测试需要有 Explorer 的交互 Windows 桌面：

```powershell
.\scripts\Test-P3.ps1
```

桌面测试会短暂创建合成窗口、移动鼠标并点击测试窗口；运行期间请避免操作桌面。自启动测试仅使用 `Software\QuotaRail\Tests` 临时键。公共 CI 不能代替任务栏可见性、混合 DPI、登录启动和长期稳定性实测。桌面测试曾出现焦点/时序波动，请保留首次失败结果，不以反复重跑代替诊断。

## Pull Request

- 每个 PR 解决明确问题，说明原行为、改动后行为与验证结果。
- 额度逻辑覆盖窗口缺失、未知时长、异常与历史边界，不用 0% 掩盖缺失。
- 界面修改提供虚构数据预览，并注明 Windows build、DPI 和布局。
- 仅提交源码、合成测试、资源和公开文档。个人设置、缓存、认证、日志、内部规划和原始采样留在本地。
- 提交前运行 `git diff --check` 并检查暂存内容；`.gitignore` 不清除旧历史。
- 保持认证由 Codex CLI 管理，不新增令牌解析或私有 HTTP 回退。

安全问题参见 [SECURITY.md](SECURITY.md)。提交贡献表示你有权按 MIT 许可证提供贡献。
