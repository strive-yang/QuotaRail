# 维护者发布流程

## 发布内容

公开源码包含 Rust 源码、合成测试、构建脚本、图标、公开说明和 GitHub 配置。`Cargo.lock` 应提交。内部规划、阶段报告、账号、设置、缓存和原始诊断数据不进入 Git 或 Release；公开文档不可链接到未公开的内部材料。

`.gitignore` 阻止误跟踪，`.gitattributes` 的 `export-ignore` 约束源码归档，两者都不能从旧提交中删除文件。首次发布使用审阅过的全新公开历史，后续只从公开仓库推送；不要将包含内部资料的开发历史合并到公开分支。

## 准备版本

1. 同步 `Cargo.toml`、`Cargo.lock`、README、CHANGELOG 和 `docs/releases/vVERSION.md`。
2. 在 Windows 运行 `scripts/Test-CI.ps1`；在可安排的交互桌面运行 `scripts/Test-P3.ps1`。记录未测范围，Beta 不宣称全矩阵稳定。
3. 从干净提交运行打包，并解压核对可执行文件、说明、许可证和校验和。

```powershell
.\scripts\Package-Release.ps1 -ExpectedTag v0.1.0-beta.1
```

默认输出 `artifacts/releases/`。打包从独立 target 目录构建，包含 `quotarail.exe`、诊断工具 `quota_probe.exe`、用户文档、图标、本项目许可证及已解析依赖的许可证文本。不会打包 mock 程序、PDB、个人数据或内部文档。重复打包请指定新的 `-OutputDirectory`，避免覆盖待审阅资产。

## 发布

在公开仓库中确认工作区干净、CI 通过，再创建与 Cargo 版本一致的标签：

```powershell
git tag -a v0.1.0-beta.1 -m "QuotaRail v0.1.0-beta.1"
git push origin main
git push origin v0.1.0-beta.1
```

`Release` workflow 从标签的干净 checkout 运行检查、重建和打包，检查 tag/version、说明和校验和。发布 job 才授予 `contents: write`；带 `-` 的版本标记为 prerelease。已存在 Release 时工作流会失败，维护者应检查现有内容，避免静默替换下载文件。

当前 CI 和发布不启用持久缓存；仅复用 Cargo 单次任务内的构建目录。未来若增加缓存，仅限 registry 源码/依赖索引等构建资料；禁止缓存整个用户目录、带凭据的 Cargo 配置、`CODEX_HOME`、QuotaRail 数据或诊断输出。发布构建不恢复 PR 编译产物。

Actions 使用从官方仓库核验的完整提交 SHA。仓库设置中启用私密漏洞报告、PR 所需检查与适当分支保护；这些设置由 GitHub 服务端管理，不由本地 YAML 自动保证。
