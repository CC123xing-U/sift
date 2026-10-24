# DECISIONS.md —— 小决策记录（Orchestrator 所有）

> 格式：日期 · 决策 · 理由。影响架构的决策已升级向用户确认。
> 修改 DESIGN.md 第 2 节写死的决策前必须先问用户（DESIGN §0.9）。

## 2026-10-24 第一轮 · 契约冻结

1. **git 初始化**：仓库原本不存在 git；契约冻结前 `git init -b main` 并纳入全部现有文件（`docs/`、参考图）。
2. **产品名统一为 Sift**（用户 2026-10-24 确认）：workspace 包名 `sift-*`、crate 前缀保持 `ff-*`（DESIGN §3 与分工表已写死）；DESIGN.md 中产品名残留处（标题、§3 布局目录名、§6 索引目录、§10.1 管道名）由 FlashFind 改为 Sift；§1.3「旧版 FlashFind 基线 69 秒」与 §16 开放项 2 为历史数据/旧代码引用，保留原名。
3. **时间戳内部统一为 Windows FILETIME**（i64，100ns，1601 纪元）：NTFS/USN 原生格式，免转换损失；IPC/UI 边界（M3/M6）再转 unix 毫秒。DESIGN §4.1 原文留「FILETIME 或 unix 微秒，选一个并全局统一」。
4. **`VolumeId` 定义在 ff-ntfs**（serial + drive，序列号为稳定键）；**`VolumeInfo` 归 ff-core**（§4.1 要求 `VolumeIndex.volume: VolumeInfo`，且 ff-core 不得依赖 ff-ntfs）；两者由 ff-engine 在 M2 映射。DESIGN 未明确 VolumeId 归属。
5. **`VolumeScanner::read_changes` 属 M2**：契约按 §5.1 冻结含该方法；Track A 本轮以 `ScanError::Unsupported` 显式占位，不留空实现。
6. **ff-core / ff-match 设 `#![forbid(unsafe_code)]`**：落实 DESIGN §0.5 平台隔离与纯逻辑要求；确需 unsafe 时走 `docs/REQUESTS.md`。
7. **flags 位标志手写常量**（不引入 bitflags crate：不在 DESIGN §2.1 清单内）。
8. **ff-cli 参数解析手写**（不引入 clap）：本轮只有 scan 一个子命令；search/index/dump 留下轮。若后续嫌糙，提理由由 Orchestrator 问用户。
9. **bench / tests 设为 workspace 成员包**（`sift-bench` / `sift-tests`）：根为虚拟 manifest，集成测试与 criterion 基准需要包载体；DESIGN §14 的 `tests/windows_integration/` 对应落点为 `tests/tests/windows_integration.rs`（包内 tests 目录）。
10. **依赖登记流程**：各 Track 不直接写 `docs/DEPENDENCIES.md`（避免并行写冲突），在汇报中列出；Orchestrator 合并时统一登记。版本由引入方核对 docs.rs 后确定。
11. **worktree 位置**：`D:/Sift/.worktrees/<track>`（分支 `agent/<track>`），已加入 `.gitignore`；各 agent 只改自己的目录。
12. **BENCH.md 汇总方式**：Track D 写合成数据结果；Track A 的真实卷 M0 数据由 Orchestrator 在合并时追加（A 的原始数据在 `crates/ff-ntfs/NOTES.md`）。
13. **UI 参考文件路径**：用户已将参考图移至 `docs/ui-reference/`；`REFERENCE_NOTES.md` 不存在，「喜欢/不要」以用户口头偏好为准（不要左侧导航栏、简约紧凑、字体搭配完美）。
14. **本轮 UI 范围**：Track C 只交付 `docs/UI_SPEC.md` 并停下等批准（用户工作流 §5 第 2 步）；前端原型（`app/`，vite + React + TS + MockEngine）批准后再做。`app/` 本轮不进 Rust workspace（Tauri 外壳属 M3）。
15. **engine 实现不在本轮**：ff-engine 本轮只冻结 `api.rs`；`InProcess` 实现、Base+Overlay、落盘属 M2，因此 ff-cli 的 `search` 子命令也留下轮。

## 合并期追加

（合并时记录：依赖登记、BENCH 数据来源、修复记录等）
