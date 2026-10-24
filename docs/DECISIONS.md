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

## 第三轮 · 文档合规审查出的 Blocker 修复（2026-10-24，`fix/ntfs-gaps`）

> 来源：`doc-audit` 对照 DESIGN.md 的审查结论（B1/B2/B3 + 文档过期）。本轮只修 Rust 侧与文档，不改冻结契约类型。

31. **USN 读错误码分派放进 `usn` 纯逻辑**：新增 `win_err::{ERROR_HANDLE_EOF=38, ERROR_MORE_DATA=234, ERROR_JOURNAL_ENTRY_DELETED=1397}`、`ReadOutcome` 与 `classify_read_outcome(code)`（`crates/ff-ntfs/src/usn.rs`），`win32.rs` 只做分派。理由：这是纯数值判据，放纯逻辑模块才能跨平台进 CI 单测（沿用决策 26——真实 IO 不进 CI）。
32. **`ERROR_HANDLE_EOF` = 干净结束（DESIGN §5.3）**：`read_usn_journal` 遇 EOF 返回**已累积的记录 + 最后已知的续读位置**（`Ok`），不再当错误；一段都没读到时续读位置回退为请求起点（`from_usn`），语义是"没有新东西可读"——这是增量轮询最常态路径，此前会抛 `ScanError::Io` 导致 `read_changes` 永远失败。
33. **`ERROR_JOURNAL_ENTRY_DELETED` → `ScanError::JournalUnavailable`**：让上层按 §5.3 重建条件（丢弃增量、全量重建）处理，而不是笼统的 Io 错误。
34. **Win32 码提取不再靠掩码猜**：`win32_code()` 取 `HRESULT` 低 16 位后**用 `HRESULT::from_win32` 反验**（`windows-result` 定义 `(error & 0xFFFF) | (7<<16) | 0x8000_0000`），`debug_assert` 保证取法正确；依据写进注释。
35. **引入 `tracing 0.1.44`（DESIGN §2.1 准入，无需再批）**：V3（ReFS 128 位 FRN）跳过、未知版本跳过、解析提前停止（`stopped_at`）三处改为 `tracing::warn!` 输出，落实 §5.3「记录日志并忽略」。`ff-ntfs` **只依赖 `tracing`**，订阅者（`tracing-subscriber`）归上层 `app` / `ff-service`（M3/M6），避免库替应用决定日志输出方式。已按 §0.2 登记 `docs/DEPENDENCIES.md`。（注：此前计数被 `scanner.rs` 静默丢弃。）
36. **建索引阶段应用最小排除集**：`$RECYCLE.BIN`、`System Volume Information`——**只按目录名匹配、大小写不敏感**（`is_excluded(name, is_dir)`），避免误伤同名文件；依据 DESIGN §5.2「排除规则在建索引时应用」。**预期影响**：真实卷条目数会下降（回收站内容不再入索引），后续 BENCH 的真实卷数字应以此为基准，不要与修复前的 849,366 / 878,732 直接比较。
37. **`SearchOptions::hide_system` 接通**：此前是死字段（只有 TS 侧镜像）。现在 `InProcessEngine::search` 在汇总命中时过滤 `flags::SYSTEM` 条目（默认 `true`），落实 §5.2「默认可在设置中隐藏」这条路。
38. **父 FRN 也要过 `mask_frn`**：`usn::record_to_change` 生成的 `RawEntry.parent_frn` 此前未掩码（只掩码了自身 `frn`），与 §5.3「用作键时统一 `& 0x0000_FFFF_FFFF_FFFF`」不一致；已改并补单测。
39. **清理"文档说没做、其实做了"的过期口径**：`ff-ntfs/src/lib.rs`（`read_changes` 属 M2 / 返回 Unsupported）、`scanner.rs` 模块头、`usn.rs` 模块头（"M2 落地时二选一"）、`tests/tests/windows_integration.rs`（M1 口径注释、测试名、`TODO(a-ntfs)`）。`volume_enumeration_smoke` 保持 `#[ignore]`，TODO 改为"管理员终端手动运行 `cargo test -p sift-tests -- --ignored` 时扩充为真实调用"。
40. **CLI 逐步补齐（补审计 Minor 19）**：当前只有 `scan` + `search`；`index` / `bench` / `dump`（DESIGN §3）属后续里程碑，各自随对应里程碑落地。
41. **`FRN_MASK` 在 `ff-core`（`entry.rs`）与 `ff-ntfs`（`usn.rs`）各定义一份**：已知重复，暂不收敛——已有跨 crate 单测 `frn_mask_low_48_bits_matches_ff_core` 守卫一致性，收敛需引入共享常量 crate，收益低于成本。
42. **chips 与 M4 修饰符的阶段错配 —— 取 A（UI 侧标注 + M3 前置禁用），本次不动引擎**：UI_SPEC §3.4 的 7 个 chips 写入 `ext:` / `folder:`，而 DESIGN §7.2（`DESIGN.md:258`）把二者列为 **M4 必做**，M1/M3 引擎不支持。点 chip 会命中 `ff-core::query` 的 `ParseError`（"修饰符语法尚未支持，计划 M4"），经 `SearchSummary::parse_error` 由 UI 弹**明确的语法错误条 + 0 结果**——是 M4 未实现的正常表现，**不构成 DESIGN 违背**（§7.3 的"容错降级"针对的是"正在打字的半成品输入"，实现已对未闭合引号降级）。当前原型由 MockEngine 自行实现这两个修饰符，故 chips 在原型里可用（有意，非越界实现）。
    **M3 接真实 Tauri 引擎前必须二选一**：① chips 置灰并说明原因（与 §3.4 删「全文」留 M7 同一先例）——**当前取 ①**；② 把 `ext:` / `folder:` 提前进 M3 引擎——属里程碑范围调整，需用户确认。**M3 开工前须复核本条。**
43. **新增 `SearchOptions::hide_hidden`（team-lead 裁定走 ①）**：决策 37 接通的 `hide_system` 只过滤 `flags::SYSTEM`（0x4），而 DESIGN §11.3 / UI_SPEC 设置项写的是一项「隐藏系统/隐藏文件」——只带 `FILE_ATTRIBUTE_HIDDEN`（0x2）的文件仍会出现，开关拨了看不到变化。**当前形态：一个 toggle 同时驱动 `hide_system` 与 `hide_hidden`**，二者默认均为 `true`。引擎侧写两个**独立**判断（`inprocess.rs`），不合并成 `SYSTEM | HIDDEN` 掩码常量——M4 若把 UI 拆成两个 toggle，各映射一个字段即可，引擎不必再动。`hide_system` 的默认 `true` 与「`hide_system = !UI开关`」的取反方向**保持不变**（已核对：引擎默认 true ↔ UI 开关默认 false）。

### 第三轮 · 文档复验修正（doc-verify，2026-10-24）

> 来源：第三轮的独立复验（换一批眼睛复核 DESIGN §13 的 M0/M1/M2 验收项，并全仓搜索文档 / 注释与代码的矛盾）。
> 本轮只改 `.md` 与注释，**未改任何 `.rs` / `.ts` 逻辑代码**；门禁（`cargo fmt --all -- --check` / `cargo test --workspace`）保持绿。
> 编号接上文（#43 由 team-lead 的 `hide_hidden` 条目占用），故本节从 #44 起。

44. **`crates/ff-ntfs/NOTES.md` 开头口径更正**：原文「`read_changes` 返回 `ScanError::Unsupported`，未实现 USN 增量」已过期——决策 25 之后 `NtfsVolumeScanner::read_changes`（`scanner.rs:148`）已真实实现。改为保留 M0 摸底原文 + 加注「已实现，但尚未被 `ff-engine` / `ff-cli` 接线」。理由：NOTES.md 是 M0 当时的实测记录，直接改写会让历史口径失真，故只加注。
45. **`crates/ff-engine/src/lib.rs` 模块头重写**：原文「本轮（第一轮）只冻结 `api` 契约；引擎实现（卷管理、Base+Overlay、后台合并、落盘调度、`InProcess` 实现）属 M2，不在本轮范围」已过期——`inprocess.rs` 已交付 `InProcess`（`search` / `range` / `cancel` / `status` / `rescan` / `open` 全通）。改为「已落地 / 仍未落地」两段，并点明 `EngineStatus::last_saved` 恒为 `None`（无落盘）。理由：模块头是判断「这块做没做」的第一入口，过期描述比缺失更有害。
46. **`bench/src/lib.rs` 模块头删除过期 `TODO(b-core)`**：`SynthEntry::into_entry_input` 从未存在、也不再需要——`build_index`（`bench/src/lib.rs:485`）直接构造 `ff_core::EntryInput`；改为指向 `build_index`。理由：Track B 早已交付 `EntryInput`，TODO 指向一个不存在的函数。
47. **`tests/tests/windows_integration.rs` 的 `volume_enumeration_smoke` ignore 理由**：去掉「卷枚举 API 待 Track A 提交后启用」——`ff_ntfs::enumerate_volumes()` 已交付且已被 `ff-cli` 使用（同文件注释第 111 行亦如此声明）；改为「卷枚举的真实调用只在此手动模式下进行」。理由：ignore 的真实理由是需管理员权限，不是 API 未交付。
48. **DESIGN.md §0 与 §3 的路径更正**：`docs/reference/` 目录不存在，三份参考文档实际在 `docs/` 根（`01-Everything-功能实现文档.md` / `02-Listary-功能实现文档.md` / `03-重写指南-取长补短.md`）；§3 目录树同时补上 `ui-reference/`（界面参考图）。理由：引用不存在的路径会让新读者找不到背景文档。
49. **DESIGN.md §3 的 ff-cli 子命令列表更正**：由「index / search / bench / dump」改为「scan / search（已实现）；bench / dump 待后续里程碑（见 DECISIONS #40）」。理由：`index` 从未实现，全量扫描摸底实际由 `scan` 承担（决策 40），`bench` / `dump` 亦未落地。
50. **`docs/DEPENDENCIES.md` 依赖登记更正**（版本已与 `Cargo.lock` 逐条核对，8 个 crate 全部一致）：`memchr` 的实际引入 crate 是 `ff-match`（原登记为 `ff-core`）；`rayon` 补登 `ff-match`；`rustc-hash` 补登 `ff-ntfs`（`scanner.rs` 的 FRN 集合 / 父计数）；`windows` 补登 `sift-bench`（`memprobe` 的 `GetProcessMemoryInfo`）。理由：DESIGN §0.2 要求登记簿与实际依赖一致，否则许可证与影响面排查会漏。
51. **`bench/Cargo.toml` 注释更正**：删除「版本范围遵循 DESIGN §2.1 的 `>=0.59, <=0.62` 约定」——DESIGN §2.1 只规定用 `windows` crate，从未规定版本范围（全仓检索无此约定）。理由：引用不存在的约束会让后续维护者误以为存在版本上限。
52. **决策 29 的记录更正（历史条目不改，此处更正）**：`VolumeIndex::highlight_ranges` 实际返回 `Vec<Range<u32>>`（`crates/ff-core/src/highlight.rs:21`），不是决策 29 写的 `Vec<Range<usize>>`；与 `ff_engine::api::ResultRow::name_matches: Vec<Range<u32>>`（`api.rs:107`）一致。
53. **M2 复验结论（代码缺口，非文档可修，须用户确认是否补做）**：DESIGN §13-M2 的验收项里只有两项落地——排除规则（`scanner.rs:26`，决策 36）与 `read_changes` 真实实现（`scanner.rs:148`，决策 25）。持久化（§6）、Base+Overlay（§4.3）、后台合并、`read_changes` 的调用方、以及「journal id / next_usn 失效触发全量重建」的端到端行为**均未实现**：全仓无 `FFIX` / `zstd` / 落盘代码；`ff-engine` 与 `ff-cli` 没有任何 `read_changes` 调用点；`read_changes` 内部的失效判定（`scanner.rs:158` / `scanner.rs:165` 返回 `JournalUnavailable`）无人消费。`BENCH.md` 因此没有任何 M2 数据。理由：如实记录，避免把「库层已实现」当成「里程碑已达成」。
54. **代码缺口：`InProcessEngine` 未实现 DESIGN §9 的「60 秒无访问释放命中集合」**（`api.rs:124` 引用该口径）：当前只在发起新搜索时 `clear()`（`inprocess.rs:161`），`cancel` 只置取消位，命中集合无超时回收，长会话下 `results` 只增不减。建议落点：`crates/ff-engine/src/inprocess.rs::search_impl` 或后台清理线程。
55. **代码缺口：`SearchOptions::fuzzy` / `pinyin` 仍是死字段**（`api.rs:55-57` 声明默认 `true`，但 T2 / T3 属 M5，`ff-match` 尚无拼音 / 模糊匹配器，`search` 未读这两个字段）。与决策 37 修复 `hide_system` 是同类问题。建议：M5 接线时一并接通；若 M5 前想避免误导，可把字段注释改为「预留，M5 生效」。
56. **可选改进（本轮未改）**：DESIGN §13-M0 要求「列出 `ntfs-reader` 的限制」，限制清单实际在 `crates/ff-ntfs/NOTES.md:33-40`，`BENCH.md` 的 M0 节（`BENCH.md:16`）只给了指引、正文未列。本轮不搬运，避免两处口径漂移；如希望 BENCH.md 自包含，可在其 M0 节补一行摘要 + 链接。

### 第三轮 · 收尾修正：未接线开关的里程碑标签错配（2026-10-24，`fix/pending-labels`）

> 来源：doc-audit 在 #55 之后指出——上一轮 `15c9673` 给三个未接线 toggle 统一打了「M3 接线后生效」常量是**错的**：`模糊匹配` / `拼音匹配` 属 **M5**（T2 / T3，DESIGN §8.1 / §13），不是 M3。若统一标 M3，M3 交付时这两个项会带上**新的谎言**，且用户会误以为 M3 已提供模糊/拼音。

57. **未接线开关按真实里程碑分别标注**：`Settings.tsx` 的 `Toggle` 组件原先用单一常量 `PENDING_NOTE = 'M3 接线后生效'` 套在所有三个 toggle 上。改为 per-toggle 的 `pendingNote?: string` 属性：
    - `显示系统/隐藏文件` → `pendingNote="M3 接线后生效"`（T 属 M3，正确）；
    - `模糊匹配` / `拼音匹配` → `pendingNote="M5 生效"`（T2 / T3 属 M5）。
    同步更新 `UI_SPEC.md` §4.8.1：① 表格拆成「M3」与「M5」两行；② 新增硬性规则「**不得统一套用同一个里程碑标签**——每个未接线项必须标出它真实的生效里程碑，以 DESIGN 的里程碑归属为准」；③ §6.3 小结的表述由「三个 toggle 都标 M3」改为「分别标 M3 / M5」。
    **验证**：`tsc -b` + `vite build` 绿；产物 bundle 中 `M3 接线后生效` ×1、`M5 生效` ×2，确认分别进了构建产物。commit `c4ead97`，合并 `4950a91`。
    **与 #55 的关系**：#55 把引擎侧 `fuzzy` / `pinyin` 注释降级为「预留，M5 生效」；本条把 UI 侧标签对齐为同一个 M5，两处口径一致，杜绝"引擎说 M5、界面说 M3"的矛盾。

## 第四轮 · M2 持久化 + 增量落地（feat/m2-persistence，2026-10-24）

> 来源：M2-②/③/④ 实现收尾（DESIGN §13-M2）。M2-① 持久化格式已于 `05a9509` 落地（零依赖 FFIX + 自 CRC32 + 原子写 + 容错 load），本轮补齐 Overlay / 后台轮询 / JournalUnavailable 重建 / 60s 回收 / 落盘接线。

58. **持久化零依赖（不引入 serde / zstd / crc 第三方 crate）**：FFIX 格式手写二进制布局，`crc32` 自实现（查表法），写用「临时文件 + `rename`」原子提交，载入做长度/魔数/CRC 容错（损坏返回 `None` 让上层重建）。理由：DESIGN §2.1 准入且 §6 明确「优化路径（rkyv + mmap）M2 之后才考虑」，压缩（zstd）与零拷贝（rkyv/mmap）均**显式推迟**到后续里程碑；零依赖让 `ff-core` 维持平台无关、可跨平台编译/测试（与决策 6 的 `forbid(unsafe_code)` 同目标）。已在 `docs/DEPENDENCIES.md` 确认本轮未新增 crate。
59. **`InProcessEngine` 全量重写落地 M2 全部行为（保留 15 个旧单测 + 新增 4 个）**：
    - M2-② Base + Overlay：`Overlay { added_or_changed: FxHashMap<u64,OverlayEntry>, deleted: FxHashSet<u64> }`（`overlay.rs` 新模块）；`search` 合并 Base 命中（跳过 `overlay.deleted`、被 overlay override 的 frn）与 Overlay 命中（`overlay_matches` + `flags()`），`resolve_row` 走 `overlay_full_path` / `overlay_highlight`，`stale_meta` 时 `size`/`mtime` 置 `None`。
    - M2-② 后台轮询 + 落盘：`background_loop` 每 100ms slice 调 `poll_all`（读 `read_changes` → `overlay.apply` → 更新 `next_usn`；`overlay.len() > 50,000` 触发 `merge_overlay_into_base`）→ `maybe_save`（`SAVE_INTERVAL=10s` 节流）→ `reap_stale`（`RESULT_TTL=60s` 回收命中集，落实决策 54）；`status().last_saved` 由 `now_filetime()` 填入。
    - M2-③ `JournalUnavailable → rebuild`：`poll_volume` 捕获 `Err(ScanError::JournalUnavailable(_))` 调 `rebuild_volume`（丢弃 overlay + 用记录点 `journal_id/next_usn` 全量重建）；端到端由单测 `journal_unavailable_triggers_rescan` 覆盖。
    - M2-④ 落盘接线：`save_volume`/`save_all` 写 `<serial>.ffix`，`load_all` 启动时读回；`set_save_dir` 由 `ff-cli`/上层设置。
60. **`empty()` 不 spawn 后台线程 + `FakeScanner` 确定性驱动，规避后台线程竞争**：`InProcessEngine::empty()`（测试构造器）**不启动** `background_loop`，改暴露 `set_scanner(Box<dyn VolumeScanner>)` + `poll_once(vid)`，使单测能向 `FakeScanner`（持有 `changes/next_usn/full/unavailable` 字段）注入合成变更并**确定性**触发 `apply`/`rebuild`，无后台线程与时间竞争。`FakeScanner::read_changes` 在 `unavailable=true` 时返回 `JournalUnavailable`。4 个新增单测：`overlay_upsert_reflected_in_search`、`overlay_remove_excluded_from_search`、`journal_unavailable_triggers_rescan`、`engine_persistence_roundtrip`。
61. **门禁全绿（提交前）**：`cargo fmt --all -- --check` 干净；`cargo clippy --workspace --all-targets -- -D warnings` 退出 0（修掉 persist.rs 8 处 `redundant_closure` + `manual_is_multiple_of`、entry.rs `too_many_arguments`）；`cargo test --workspace` 退出 0（ff-engine 19 个单测全过，含 4 个新增）。**M2 端到端实机项（真实 journal 增量延迟 ≤1s、重启载入 ≤0.5s、强制 kill 后仍能载入）需管理员 + 真实 NTFS 卷，不进 CI（沿用决策 26），留待 M2 收尾实机复核。**
