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

## 合并期追加（2026-10-24 第一轮收尾）

16. **Track B 三处 API 偏离冻结片段，Orchestrator 审查后接受**：
    - `IndexBuilder::new(VolumeInfo)`：构造函数接收 `VolumeInfo`（而非冻结片段里未定的入参）；理由：索引必须携带卷元数据，且 `VolumeInfo` 已在决策 4 归 ff-core。
    - `write_path(&self, id, buf: &mut Vec<u8>) -> bool`：返回 `bool`（路径成功写出为 true，遇环/越界为 false），与 DESIGN §4.2「路径现算、失败可降级」一致；原冻结片段未规定返回值。
    - `highlight_ranges(id, &q) -> Vec<Range<usize>>`：作为 `VolumeIndex` 的方法（而非独立函数），与 `search`/`write_path` 同簇；理由：高亮依赖索引内部 arena，方法形态更内聚。
    - 上述三点均为**实现形态**调整，未改动跨 crate 契约类型（`Query`/`ParseError`/`EntryId`/`NONE`）。
17. **`CancelToken` 对齐冻结签名**：Track B 初版为独立 `AtomicBool`，合并前改为 `pub struct CancelToken(Arc<AtomicBool>)`，与 `ff-engine::api` 的共享取消语义一致；已在 `ff-core::cancel` 导出。
18. **依赖登记**：见 `docs/DEPENDENCIES.md`（ntfs-reader / windows / rayon / memchr / rustc-hash / criterion 全部登记）。
19. **BENCH 数据来源**：合成数据 2M 实测由 Track D 记录于 `docs/BENCH.md`「M1 合成数据」；真实卷 M0（C: 849,366 条 / 5.312s）由 Orchestrator 追加于同文件「M0 真实卷摸底」，原始明细见 `crates/ff-ntfs/NOTES.md`。
20. **合并顺序与冲突处理**：按 A→B→D（C 已先入 main）顺序合并；A、D 均因 `Cargo.lock` 与 main 冲突，统一以 `cargo generate-lockfile` 重新解析（无语义冲突，仅依赖集合并集）。ff-core/ff-match 由 B 独家拥有，D 的覆盖层已剔除后才合并（见下条）。
21. **Track D 的 ff-core/ff-match 覆盖层已剔除**：D 的分支在 B 之前切出，为编译基准曾把 B 的 ff-core/ff-match 源码整块拷入工作树；合并前已用 `git checkout agent/b --` 取权威版本验证通过，再把覆盖层 `reset`+`clean` 还原，仅提交 `bench/`+`tests/`+`BENCH.md`+`Cargo.lock`，未污染核心 crate。

## 第二轮 · InProcess 引擎 / USN 增量 / CLI（2026-10-24）

> 本轮用双 agent 并行：Track E（InProcess 引擎）与 Track M2（USN 增量）互不依赖、同时开工；Track F（`ff-cli search`）依赖 E，由 Orchestrator 在 E 合并后直接实现。全部在隔离 worktree（`.worktrees/e` / `.worktrees/m2` / `.worktrees/f`）完成后入 main。

22. **多卷命中的表示：`Vec<(VolumeId, EntryId)>`**：命中标明所属卷，`range`/`open` 据此回到对应 `VolumeIndex` 取字段。契约（`EngineClient` trait）未规定内部形态，故由实现自选；已要求 `InProcessEngine` 保持跨平台（无 `cfg(windows)` 外的平台代码）。
23. **Score 排序在 M1 退化为 entry_id 升序**：DESIGN §8.2 评分不在 M1 范围，无分数可排；Name/Path/Size/Modified 均已按真值排序。待 M2/M5 引入评分后补齐，届时 Score 降序语义回归。
24. **`open` 语义：InProcess 下 `CopyPath` 暂返回 `Ok(())`**。理由：复制到剪贴板是 UI 层行为（DESIGN §12 由 UI 进程执行），引擎只负责解析出完整路径；Open/OpenFolder 在 Windows 下确有 `explorer` 行为。非 Windows 分支返回 `Ok(())`，保证跨平台可编译。
25. **`read_changes`（M2）真实实现已落地**：`FSCTL_READ_USN_JOURNAL` + `READ_USN_JOURNAL_DATA_V0` 经 `DeviceIoControl` 读原始记录流，按 `RecordLength` 逐条解析。USN 记录**不含文件大小与可靠时间戳**（已知 Windows 行为），故生成的 `RawEntry` 置 `size=0`、三时间戳统一取记录 `TimeStamp`（FILETIME），消费方应按 DESIGN §4.3 标记 `STALE_META` 后台补齐。
26. **USN 实时路径不在 CI 验证**：真实 `read_changes` 需管理员权限 + 真实 NTFS 卷 + 已启用 Journal，CI 不满足。**只允许纯逻辑单测**（合成 `USN_RECORD` 字节 → `Change`）进 CI；不得新增依赖真实 IO 的 `#[test]`。
27. **ff-cli 参数解析继续手写**（沿用决策 8，不引入 clap）：本轮子命令增至 2 个（scan/search）仍属简单，`--max/--sort/--asc/--case` 手写解析并把**跨平台解析函数**与平台相关执行分离，使参数解析可在任意平台被单测覆盖。
28. **多个位置参数拼成一个查询串**：`ff-cli search C: my docs` → 查询串 `my docs`；理由是查询语法里空格即 AND，与 UI 输入行为保持一致（用户直接输入即 AND）。
29. **本轮合并无 `Cargo.lock` 冲突**：E、M2 均未改动各自 `Cargo.toml`（ff-engine 早已依赖 ff-core/ff-ntfs；windows features 已含 `Win32_System_Ioctl`），F 复用了已存在的 ff-cli→ff-engine 依赖。合并顺序 E→M2→F。
30. **Track F 由 Orchestrator 直接实现而非再派 agent**：该任务紧耦合刚落地的 `InProcessEngine` API，上下文已在手，且依赖顺序上无法与 E 并行；并行收益低于往返成本。
