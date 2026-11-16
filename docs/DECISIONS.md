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

## 第五轮 · M3 可用 UI（Tauri 外壳 + InProcess 引擎，2026-10-24）

> 来源：M3 落地——`app/src-tauri`（Tauri v2 独立 crate）+ 前端 `TauriEngine`（替换 MockEngine 原型）+ 引擎契约 serde 化（M3①）。DESIGN §13-M3 要求「Tauri 窗口 + 全局热键 + 托盘 + 虚拟滚动 + 基本快捷键；InProcess 实现的 EngineClient」。

62. **Tauri v2 外壳整体落地（文件级清单）**：
    - `app/src-tauri/Cargo.toml`：独立 `[workspace]`（不并入根 Rust workspace，避免成员列表排除报错），path 依赖 `ff-core/ff-match/ff-ntfs/ff-engine` + `tauri`/`tauri-plugin-global-shortcut` v2 + `serde`/`serde_json`。
    - `app/src-tauri/tauri.conf.json`：`withGlobalTauri: true`（前端零 `@tauri-apps/api` 依赖）、窗口 `760×480`、无边框、alwaysOnTop、`theme: Auto`、`frontendDist: ../dist`、`devUrl: http://localhost:5173`、托盘图标 `icons/icon.png`。
    - `app/src-tauri/src/main.rs`：定义 9 个命令（`search`/`range`/`cancel`/`status`/`rescan`/`open` + `show_window`/`hide_window`/`toggle_window`），`setup` 注入 `Arc<InProcessEngine>`，托盘菜单（显示/隐藏/重建索引/退出），`on_window_event` 失焦自动隐藏，`generate_handler!` 注册全部命令。
    - 引擎接线（Windows）：`build_engine()` 用 `NtfsVolumeScanner` 构造 `InProcessEngine`，后台线程 `enumerate_volumes()` 逐卷 `rescan(Some(vid))` 触发全量扫描（不阻塞窗口）；非 Windows 走 `InProcessEngine::empty()` 仅保证可编译。
    - 前端：`app/src/engine/TauriEngine.ts` 用 `window.__TAURI__.core.invoke` 实现 `EngineClient`，`App.tsx` 按 `isTauri()` 在 `TauriEngine` 与 `MockEngine` 间切换；`status()` 改为 `async`（Tauri/IPC 异步一致性，同步改了 `types.ts`/`MockEngine.ts`/`App.tsx` 的轮询 `useEffect`）。
    **M3① serde 契约**：`ff-core::ParseError.span: Range<usize>`、`ff-engine::ResultRow.name_matches: Vec<Range<u32>>` 经本地 wire 适配器（`serde_range`/`serde_ranges`）保留 Rust `Range` 类型且 IPC 形态为 `{start,end}` / `[[start,end],...]`；枚举 `snake_case` 重命名（`Name`→`name` 等）；`VolumeStatus.volume` 由 `VolumeId` 改为 `String`（IPC 边界只展示）。`cargo test -p ff-engine` 19 单测全过。
    **验证**：前端 `npm run build`（tsc -b + vite build）绿；`cargo test -p ff-engine` 19 单测绿。

    **已知限制（诚实记录，非「已达成」）**：
    - **`rescan(volume)` 忽略盘符参数、统一重建全部**：引擎公开 API 仅支持「按 `VolumeId` 全量重建」或「全部重建」，无「按盘符查卷」查询；真实 UI rescan 按钮不带参数（重建全部），故行为正确但粒度粗。单卷重建需引擎暴露卷列表查询，留待后续。
    - **双击 Ctrl（`WH_KEYBOARD_LL`，DESIGN §11.2）未实现**：M3 仅做 `Ctrl+Alt+Space` 全局快捷键（`tauri-plugin-global-shortcut` → `toggle_window`）+ `Esc` 隐藏真实窗口；双击 Ctrl 是 Windows 低级键盘钩子，复杂且易引入不稳定，留待后续（设置项可关）。
    - **Tauri crate 沙箱内已验证**：`cargo check` + 完整 `cargo build` 均绿——`target/debug/sift-app.exe`（15.2 MB，debug）真实链接产出（rustc 自带 MSVC 链接器可用，PATH 上无 `cl.exe` 不构成阻碍）；`cargo clippy --all-targets -- -D warnings` + `cargo fmt --all --check` 均绿。修复记录：补 `icons/icon.ico`（32×32 32bpp BMP-in-ICO，tauri-build 生成 Windows 资源必需）、开 `tauri` 的 `tray-icon` feature、main.rs 导入 `EngineClient` trait（方法在 trait 上）、`PredefinedMenuItem::separator(app)?` 与 `with_shortcuts(...).expect(...)` 两处 v2 `Result` 链修正。
    - **「按键→首屏 ≤50ms」是运行时指标**：引擎 M1 基准（substring ≤100ms @2M 条目，`BENCH.md` M1 节）可作参照，但真实 GUI 计时需跑 `tauri dev` 实机测，沙箱无法跑 GUI，留待 M3 收尾实机复核。

63. **debug exe 走 devUrl + 控制台窗口（用户实跑 M3 debug 产物发现，2026-10-25）**：
    - **现象**：用户直接运行 `target/debug/sift-app.exe` → ① 弹出控制台窗口；② GUI 窗口显示「localhost 连不上」。
    - **根因**（tauri v2.12.5 源码 `manager/mod.rs:345` 实证）：`#[cfg(dev)]` 时加载 `build.devUrl`，`#[cfg(not(dev))]` 才加载 `build.frontendDist`；`dev` cfg 由 tauri-build 在 **debug profile** 时发出（build script 输出可见 `cargo:rustc-cfg=dev`）。故 debug exe 必然请求 `http://localhost:5173`——不跑 `npm run dev` 就必然白屏。控制台同理：`#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]` 在 debug 下保留控制台子系统。
    - **修复**：`main.rs` 改无条件 `#![windows_subsystem = "windows"]`（GUI 应用不需要控制台；调试日志日后用 dbgview）；测试/分发用 **release** 构建（`cargo build --release` 或 `npm run build:tauri`），release 下 `cfg!(dev)` 为假 → 加载 `frontendDist: ../dist`（即 `app/dist`，由 `beforeBuildCommand` 生成）。
    - **结论固化**：**debug exe 不可直接分发给用户运行**；README/文档需明示「运行 release 产物或 `npm run build:tauri`」。
    - **更正（2026-10-25 后续实证）**：上条「release 下 `cfg!(dev)` 为假」的表述**不准确**。`dev` cfg 并非按 profile 判定，而是由 tauri 2.12.5 的 build.rs 依 **`tauri` 依赖是否启用 `custom-protocol` feature** 决定（`build.rs` 中 `dev = !custom_protocol`）。故手动 `cargo build --release` **仍会走 devUrl**；必须 `cargo build --release --features custom-protocol`（或经 `tauri build` CLI，它会自动开该 feature）才会加载 `frontendDist: ../dist`。已在 `app/src-tauri/Cargo.toml` 声明 `custom-protocol` feature 并转交 `tauri`。

64. **Bug #6：索引完成后无限循环重建（2026-10-26 修复 + 回归测试 + 实机验证）**：
    - **现象**：3 个卷全部 `rescan done` 后，UI 仍反复进入「正在建立索引」，CPU/磁盘持续被周期性全量重扫占用。
    - **根因**（代码级，确定）：`inprocess.rs::load_volume_with` 的成功分支写作 `Ok(_meta)`，**丢弃**了 `full_scan` 返回的 `ScanMeta`（含 `journal_id` / `next_usn`），随后 `vd.info = info` 写入的 `info` 只有 `serial`/`drive`（`journal_id=0`、`next_usn=0`）。后台 `poll_volume` 于是以 `from_usn=0` 调 `read_changes`，而 `scanner.rs` 判定 `from_usn < info.first_usn`（真实卷 `first_usn>0`）恒为真 → 恒返回 `JournalUnavailable` → `rebuild_volume` → 全量重扫 → 回到起点。**M2 的 USN 增量在真实运行中因此从未生效**，每卷每隔「扫描耗时 + 1s」重建一次。
    - **修复**：成功分支接住 `ScanMeta`，写入 `journal_id` / `next_usn`；`guid_path` 不在 `VolumeId` 中，从旧 `vd.info` 继承（否则无盘符卷的路径前缀会丢）。语义与 DESIGN §5.2 一致——`full_scan` 在扫 MFT **之前**记录的 `next_usn` 即续读起点，扫描期间的变化由续读补齐。
    - **回归测试**：`inprocess::tests::scan_meta_is_kept_so_poll_does_not_rebuild_forever`。新增 `UsnScanner` 复刻真实 Win32 语义（`from_usn < FIRST_USN` → `JournalUnavailable`）并统计 `full_scan` 次数，断言 `rescan` 后连续三轮 `poll_once` 只触发 **1** 次全量扫描。**负向对照已做**：把 `next_usn` 退回 `0` 后该测试在第 1 轮即失败（`left=2, right=1`），证明测试真正覆盖此 bug 而非空跑。
    - **实机验证**（release + `custom-protocol`，PID 10984）：3 卷各 `rescan done` 一次、日志无重复重建；扫描结束后 30 秒墙钟内仅耗 **2.03 秒 CPU（6.8%）**，进程稳定存活（修复前 C: 卷 90 万条目每 ~6s 全扫一次，必然 80%+ 持续占用）。
    - **同批修复 Bug #7**：`save_volume` 增加 `state != Ready` 短路，避免 `Scanning`/`Error` 态的空壳索引（120 B）覆盖磁盘上的好索引（取证：10-25 14:10 的 `2661335377.ffix` 曾为 120 B）。

65. **仍待处理（承接 `AUDIT_M3_FINAL.md`，未实施）**：
    - **Bug #8（P2）`load_all` 复活幽灵卷**：无条件载入目录下所有 `.ffix`，不与 `enumerate_volumes()` 对齐 → 已卸载/被过滤的卷以 `Ready`+0 条目驻留 UI 并被反复落盘（取证：`2281921499.ffix` / `816230857.ffix` 各 120 B 被周期重写）。修法：`build_engine` 中 `load_all` 后按当次枚举的 serial 集合 prune。
    - **Bug #9（P2）落盘目录在 C: 盘**：C: 仅 430 MB 可用时，90 MB 级索引落盘会吃掉最后空间，其后 save 静默失败（`save_volume` 错误被 `let _ =` 吞掉，仅留 0 B `.tmp` 残留），且 WebView2 缓存同受牵连。属 DESIGN §6 变更，需你确认（改目录 / 允许配置 / 至少把失败写进 `VolumeData.message`）。
    - **新观察（非阻塞）**：`SAVE_INTERVAL = 10s` 且无脏标记，稳态下每 10 秒重写全部索引（本次实测 3 卷共 ~177 MB/次）。功能正确但 I/O 浪费，建议后续加 dirty 标记或拉长间隔。

66. **全局索引进度回跳 + 输入逐字检索卡顿（用户实机反馈，2026-10-26 修复）**：
    - **症状 A**：进度条「70 多 → 90 多 → 退回 70 多」反复回跳（用户原话："一直在那儿进去又退、退回又进"）；到 100% 后不再重扫（Bug #6 已修好的部分）。
      - **根因**：`overallScanProgress` 对多卷取**均值**，非扫描中的卷计 1；而卷是**逐个顺序扫描**的（`main.rs` 单线程 for 循环）。未轮到的卷索引已从磁盘载入、状态仍是 `Ready`，被算作"已完成"，于是每开扫一个新卷该卷进度归 0、全局被拽回低位。
      - **修复（两侧）**：① 引擎新增 `InProcessEngine::mark_pending(&[VolumeId])`（inherent 方法，不动冻结的 `EngineClient` trait），`main.rs` 在扫描前先把本轮要扫的卷**全部**置为 `Scanning`+progress 0；② 前端 `overallScanProgress` 改为**按条目数加权**（`w = max(1, entries)`），使 90 万条目的 C: 与 0 条目空卷不再等权。
      - **验证**：按实机卷规模（C 90 万 / D 25 万 / F 62 万 + 2 个 0 条目幽灵卷）复算曲线 —— 修复前 `49→100→85.9→100→65→100`（**2 次回跳**，与用户观感的 70~100 区间吻合）；修复后 `0→100` 单调、**0 次回跳**。实机 release 运行 3 卷各一次 `rescan done`，进程存活。
      - **约束**：`mark_pending` 只能传入确实会去 `rescan` 的卷，否则该卷会停在 `Scanning` 不恢复。
    - **症状 B**：连续输入时明显卡顿，停手才恢复。
      - **根因**：UI_SPEC §3.3 原为「每次输入变化即触发搜索」，每敲一个字都对全卷（实测约 180 万条目）做一次完整检索 + IPC 往返 + 虚拟列表重建。
      - **修复**：`App.tsx` 加 `SEARCH_DEBOUNCE_MS = 120` 防抖 —— 输入本身仍是受控 state、立即回显，只在停止输入后真正发检索；**清空输入立即执行**（不等防抖，避免结果残留）。另：被覆盖的旧结果集在回包时主动 `cancel(id)` 释放（原本要等 60s 回收）。
      - **取舍**：DESIGN §13-M3 的「按键→首屏 ≤50ms」指**引擎检索延迟**，防抖是叠加其上的 120ms 输入停顿窗口；这是与 Everything 一致的取舍，不是对该指标的破坏。

67. **M4 查询语法与属性过滤（2026-10-26 实现）**：
    - **AST 化**：`ff-core/src/query.rs` 由「扁平 AND terms」改为表达式树
      `Expr::{Empty, And, Or, Not, Name, Pred}`；手写递归下降，优先级 NOT > AND > OR。
      求值在 `search.rs`（索引路径）与新增 `eval.rs`（单条目路径，供 M2 Overlay 用）
      两处共用同一棵树——Overlay 条目此前只按 `terms` 做 AND，M4 的 OR / NOT / 修饰符
      会在增量条目上漏匹配，这是必须修的语义缺陷。
    - **契约测试随里程碑演进**：`api_m1.rs::parse_errors_surface` 原本断言
      `a|b` `!x` `(x)` `*.txt` `size:>1mb` `a\b` 为错误；M4 实现后它们全部合法，
      该测试改为「合法 + 另用真正非法输入（`size:abc` / `regex:(` / `dm:notadate` / `attrib:Z`）验证错误」。
      `ff-engine` 的 `parse_error_written_not_err` 同理改用 `size:abc`。
    - **容错优先于报错**（DESIGN §7.3）：未闭合 `(` `<`、单独的 `!`、空修饰符值
      （`size:` `dm:` `ext:` …）一律降级为字面子串或「无贡献」，只有**明确非法的值**才报错。
    - **扫描范围放宽**：`>` `<` 既是分组语法字符又是 `size:` 比较符，`|` 既是 OR 又是正则的或——
      故 `size:` `depth:`（在 `>` `<` 处不截断、在 `|` 处截断）与 `regex:`（两者都不截断）
      三个修饰符单独走放宽的值扫描。
    - **`case:` / `nocase:` 带值时**，冒号后的内容按**独立子串 term** 处理（`case:READ` =
      开区分大小写 + 搜 `READ`），否则用户写的搜索词会被当成开关参数丢掉（与 `file:notes` 一致）。
    - **日期时区**：DESIGN §7.2 要求按本地时区解释，但 `ff-core` 是零平台依赖的纯逻辑 crate，
      标准库拿不到时区偏移。方案：`Query::context.utc_offset_minutes` 由调用方注入，
      Windows 侧 `ff-engine` 用 `ff_ntfs::win32::local_utc_offset_minutes()`
      （`GetTimeZoneInformation`，`偏移 = -Bias`，夏令时再叠加 `-DaylightBias`）填真实值；
      未注入时按 UTC 日界（偏差最多 ±14 小时）。日历日换算用 Howard Hinnant 移位算法，无新依赖。
    - **`attrib:` 的能力边界**：索引的 `flags` 只保存 R / H / S / D 四个派生位，
      索引路径只能判定这四个字母；`A`（存档）`C`（压缩）`E`（加密）等其余位在索引路径判不了
      （映射后掩码为 0 即不命中）。Overlay 路径持有原始 `attrs`，可判定全部字母。
      补齐需 §4.3 增加 attrs 列，留待后续。
    - **高亮边界**：`Query.terms` 只收集子串 / 短语 term（不含通配符、正则、NOT 子树内的 term），
      因此 `*.txt` / `regex:` 在 M4 不高亮——不显示错误的高亮比显示错的更安全。
    - **性能**：全表扫描类查询原本每条名字都新建小写 `Vec`（`ext:pdf` 实测 67 ms）。
      两处优化：① `case::simple_lowercase_into` + thread-local 复用缓冲；
      ② **字面探针剪枝**——`ext:` 用 `.ext`、`*.log` 用最长字面段先走 `memmem` 出候选再验证
      （多扩展名取并集）。结果：单扩展名 67→2 ms，通配符 67→5 ms，UI chips 76→33~46 ms。

68. **M5 接线：分层匹配进入真实搜索链路（2026-11-15）**：
    - **问题**：上一轮工作树里 `ff-core::score::search_ranked` / `PinyinMatcher` /
      `FuzzyMatcher` / M5 高亮都已写好并有单测，但 `InProcessEngine::search`
      仍调 `vd.index.search()`（M4 的 AST 求值），`opts.fuzzy` / `opts.pinyin`
      在引擎里零引用 —— 模块存在，功能在 UI 上完全不生效。
    - **实现**：`inprocess.rs::search_impl` 改调 `search_ranked`，命中以
      `(VolumeId, Hit, f64)` 三元组收集（评分只在排序时用，存入 `StoredResult`
      前丢弃，`range`/`open` 的契约不变）；`StoredResult` 增加 `mopts`
      字段，保证 `range()` 高亮用与搜索同一套 T2/T3 开关。
    - **Overlay 一致性**：新增 `ff-core::score::match_overlay_name`，T1 仍由
      `Query::matches_facts`（完整 AST）判定，T1 未命中时按 `match_tokens`
      追加 T2/T3；T3 的「T1+T2 < 100」由引擎按 Base 的非 Fuzzy 命中数传入
      `allow_fuzzy`。M4 曾因 Overlay 只按 terms 做 AND 导致 OR/NOT/修饰符漏匹配，
      这次不再让增量层与 Base 分叉。
    - **高亮**：`highlight.rs` 把 M5 判定抽成自由函数 `highlight_name`，
      Base 的 `VolumeIndex::highlight_ranges` 与 Overlay 共用；`api_m1.rs`
      契约测试随签名演进补 `false,false`（只验 T1），T2/T3 契约交给 M5 用例。

69. **`SortKey::Score` 改为真实评分并尊重升降序（2026-11-15）**：
    - 旧 `sort_hits` 对 Score 用「来源序 + id 升序」近似，且**忽略**
      `sort.descending`；M5 后改为按 `f64` 评分降序（同分用来源 + id 稳定
      tie-break），`descending=true`（默认）高分在前，`false` 反转。
    - 按列排序（Name/Path/Size/Modified）仍取真值、语义与 M4 一致；
      `req.max_hits` 在排序后截断的行为不变。Top-K 部分选择未做，见 #71。

70. **生产拼音 / 模糊依赖裁定：本轮不引入（2026-11-15）**：
    - DESIGN §8.1 点名 `ib-pinyin`（T2）与 `nucleo-matcher`（T3）。按 §0
      规则 2/3，引入清单外依赖须先核对 docs.rs 接口 + 许可证 + 登记
      DEPENDENCIES，且 §15 特别提醒 nucleo 系列许可证条款需留意。本轮选择
      **零新增依赖**：T2 用 `pinyin.rs` 内置常用汉字精简字典（约 60 字），
      T3 用有序子序列匹配（语义等价 nucleo 基本行为）。
    - **代价（诚实记录）**：未收录汉字不参与拼音、无多音字、无双拼，
      DESIGN §13-M5 验收里的「多音字」未覆盖；`PinyinMatcher::pinyin_of` /
      `name_pinyin` 是唯一替换点，匹配逻辑无需改动。
    - **替换触发条件**：用户确认引入，或真实卷中文召回率不达标时。届时须补
      版本/许可证核对、DEPENDENCIES 登记、金标准用例回归与 BENCH 重测。

71. **M5 性能优化：200 万条三次实测（2026-11-15，BENCH M5 节）**：
    - 首测（工作树初态）：T2 813 ms、T3 927 ms、评分排序 978 ms，
      全部**超过** §8.1 的 150 ms 门槛，按规范必须优化。
    - 三处热点：① `score_name_bytes` 每条命中 `from_utf8_lossy().to_lowercase()`
      分配 String；② T2 串行遍历 591,778 个 `cjk_ids`，每条构造
      `PinyinTokens`（4 处堆分配）；③ T3 每条名字分配小写副本与命中位置向量。
    - 修法：评分改零分配 ASCII 不敏感比较（与 §15「不做完整 Unicode case
      folding」一致）、T2 改 rayon 并行 + `quick_may_match` 首字符必要条件剪枝
      （不改变匹配语义，单测验证）、T3 用 `FuzzyMatcher::matches_folded`
      零分配预过滤、T1 的 `sub_ranges` 不再复制整条名字。
    - 二测：T2 223 ms / T3 327 ms / 评分 463 ms；三测（剪枝后）：
      **T2 31.8 ms / T3 23.0 ms / 评分 110.3 ms**，全部达标。注意 T3 计时
      包含 T2，前两轮的主因都是拼音遍历而非模糊扫描。
    - **Top-K 取舍**：§8.2 要求 `select_nth_unstable_by` 只排 Top-K。当前
      对全部命中全量排序，78,607 命中实测 110.3 ms；若截断到 K 而
      `SearchSummary.total` 仍报真实总数，虚拟列表滚过 K 会取到空行。
      故本轮不做部分选择，等 UI 支持「结果上限」提示后再改，记为已知边界。

72. **设置页开关接线与 UI_SPEC §4.8.1 标注同步（2026-11-15）**：
    - `App.tsx` 原先一律传常量 `DEFAULT_OPTS`，设置页三个 toggle 点了只改
      React state —— 正是 §4.8.1 要防的「界面撒谎」。现按 `settings` 派生
      `searchOpts`（`useMemo`）：`fuzzy`/`pinyin` 直通，`showHidden` 成对
      派生 `hide_system = hide_hidden = !开关`（§11.3 的硬口径）。
    - 按 §4.8.1「接线后须移除对应标注」，摘掉模糊/拼音的「M5 生效」与
      显示系统/隐藏文件的「M3 接线后生效」；双拼标注「暂未支持」、
      评分规则编辑器标注「M5 预留，未实现」（真 disabled）。
    - **仍未接线**（保留 backlog，按真实里程碑逐项标注）：开机自启（M6）、
      失焦隐藏开关、双击 Ctrl（M7）、自定义热键、索引卷 / 排除路径、
      界面语言。设置项仍只存内存、不落盘（M6 服务化时补）。

73. **工作树收敛与门禁（2026-11-15）**：
    - 承接时工作树有 16 个已改文件 + 2 个未跟踪新文件，**编译不过**：
      `score.rs` 测试辅助 `dir()` 传 5 参（定义 3 参）、
      `inprocess.rs:1096` 调 `highlight_ranges` 少传 `fuzzy`/`pinyin`。
      另 `api_m1.rs` 4 处旧签名调用、`clippy -D warnings` 6 处 lint
      （`single_range_in_vec_init` / `len_zero` / `needless_borrow` /
      `iter_kv_map`）、`app/src-tauri` 一处 fmt 差异，一并收敛。
    - 修 `score.rs::shallow_beats_deep_when_scores_tie`：原测试把 `EntryId`
      误当 FRN 硬编码断言 20/21，且两个目录同深度根本没测到「浅优先」；
      改为构造浅/深两条真实路径并断言 `depth_of` 关系（负向对照：改回
      同深度即失败）。
    - 终态门禁：根 workspace fmt/clippy/`cargo test --workspace`
      **208 passed**；`app/src-tauri` fmt/clippy/check 绿；
      `npm run build` 50 modules 绿；release 实测脚本 `m5probe` 绿。

74. **P0-2 复制路径真实落地（2026-11-15，M5 收尾轮）**：
    - 旧实现 `open_impl` 里 `OpenAction::CopyPath => Ok(())` 是**静默假成功**，
      注释说「由 UI 层处理」但 `TauriEngine.open` 只是 `invoke('open')`，
      前端也没有剪贴板回退 → `Ctrl+C` 完全无效果。
    - 改为引擎侧 Win32 FFI 真正写剪贴板：`user32` 的
      `OpenClipboard` / `EmptyClipboard` / `SetClipboardData(CF_UNICODETEXT)` /
      `CloseClipboard` + `kernel32` 的 `GlobalAlloc`/`GlobalLock`/`GlobalUnlock`。
      与既有 `shell_open` 同风格（`#[cfg(windows)]` 隔离），**零新增 crate 依赖**
      （DESIGN §2.1 准入，不擅引 `arboard` / Tauri clipboard 插件）。
    - 三个易错点：① `SetClipboardData` 成功后内存所有权移交系统，**不能再
      `GlobalFree`**，只有失败路径释放；② `OpenClipboard` 可能被别的进程短暂
      占用，有限重试 5×10 ms，不长期阻塞 UI；③ 失败返回 `Err` 而不是 `Ok(())`，
      经 `TauriEngine.open` → `onOpenError` 在状态栏如实提示（P1-9 通道）。
    - 新增 `OpenAction::CopyName`（复制文件名），供右键菜单第 4 项使用；
      `file_name_from_full` 自己切 `\` / `/`，不用 `Path::file_name`，
      避免非 Windows 构建把反斜杠当普通字符。`types.ts` / `MockEngine.ts`
      同步契约。
    - 验证：`clipboard_roundtrip_writes_unicode_and_restores_previous` 单测在
      Windows 上**真实写入并回读**含中文 / emoji / 反斜杠的宽文本，结束后恢复
      原剪贴板（原内容非纯文本或剪贴板被占用时跳过，不破坏用户数据）；
      `--nocapture` 实跑回显 `clipboard roundtrip OK: 写入并回读 53 字节`。
      纯函数 `wide_with_nul` / `file_name_from_full` 另有跨平台单测。
    - 复制成功也走 `onActionDone` → 状态栏 toast「已复制完整路径 / 已复制文件名」，
      让 `Ctrl+C` 有可见反馈，而不是「好像没反应」。

75. **P0-3 右键菜单提前到 M5 收尾轮（2026-11-15）**：
    - 口径冲突：`UI_SPEC.md` §4.2 原写「右键菜单 **P2**（M7）」，
      `REVIEW_BRIEF.md` §5 却列为 **P0-3**（用户明确反馈的缺口、界面完全发现不了
      「打开文件夹 / 复制路径」）。按「用户反馈 > 排期」提前实现，同时把两份文档的
      归属同步改掉，不留下「文档说 M7、代码已做」的反向谎言。
    - 菜单四项：打开（`Enter`）/ 打开所在文件夹（`Ctrl+Enter`）/
      复制完整路径（`Ctrl+C`）/ 复制文件名，右侧直接显示快捷键，兼作
      REVIEW_BRIEF 建议的「快捷键可发现性」；状态栏在有结果时补一行
      「右键查看更多操作」小字。
    - 实现：`ResultArea.tsx` 在 `.list` 上统一 `onContextMenu`（`closest('.row')`
      取行号），菜单 `role="menu"` + `menuitem`，↑↓/Home/End/Enter/Esc 键盘可达；
      `position:absolute` 相对 `.resultarea` 并 clamp 到窗口内（浏览器原型与
      Tauri 真机坐标都对）；外部点击 / 滚动 / resize / 换结果集自动关闭；
      Esc 用 **document capture + `stopImmediatePropagation`** 抢在 App 全局 Esc
      （清空输入 → 隐藏窗口）之前，避免「关菜单顺带把窗口藏了」。
    - 抽出 `runAction` 统一键盘与菜单的动作入口，成功 / 失败都有 toast。
      **不加 hover 行内按钮**：`UI_SPEC` §4.2 的行结构是四列 grid，加按钮会改
      布局且属 P2，本轮只补 P0 的「可发现性」。
    - 里程碑归属：这轮是「M5 完成后的缺陷收尾」，不是新开 M6/M7；
      DESIGN §0「一次只做一个里程碑」仍成立。

76. **P1-1 无边框窗口拖动（2026-11-15）**：
    - `tauri.conf.json` 是 `decorations: false`，此前全仓无
      `data-tauri-drag-region`，窗口**完全不能移动**。
    - `TopBar.tsx` 给 `.topbar` 加 `data-tauri-drag-region="deep"`。
      依据本机 `tauri-2.12.5/src/window/scripts/drag.js`：deep 沿 composedPath
      上溯，遇到 `INPUT` / `BUTTON` 等可交互元素且自身未带该属性时**阻止拖动**，
      因此输入框选字、chips、✕ / ⚙ 按钮仍完全可用，只有图标与空白区触发
      `start_dragging`。UI_SPEC §3.1「输入行空白区为拖动区」由此落地。
    - **不加自绘标题栏 / 关闭按钮**（REVIEW_BRIEF P1-1 的建议方向之一）：
      `UI_SPEC` §3.1 已明确「无最小化/最大化/关闭；输入行右侧仅 ✕（清空查询）
      与 ⚙（设置）」，关闭语义由 Esc / 失焦隐藏 / 托盘退出承担，保留 Alt+F4。
      文档优先级 DESIGN > UI_SPEC > REVIEW_BRIEF，按 UI_SPEC 从简。

77. **收尾轮门禁与未覆盖边界（2026-11-15）**：
    - 根 workspace fmt / clippy `-D warnings` 绿，`cargo test --workspace`
      **211 passed**（较 M5 的 208 多 3 个：宽串编码、路径末段、剪贴板往返）；
      `app/src-tauri` fmt/clippy/check 绿；`npm run build` 50 modules 绿。
    - 构建产物自证：`app/dist/assets/*.js` 含 `data-tauri-drag-region`、
      `ctxmenu`、`copy_name`、`复制文件名`、`右键查看更多操作`。
    - **未自动化验证（如实记录）**：Tauri 真机窗口拖动、右键菜单的鼠标交互与
      剪贴板端到端 GUI 链路。浏览器原型验证时内置浏览器的截图通道持续超过
      10 MiB 限制、DOM 快照只回传结果行节点，未拿到菜单渲染后的可判定回显；
      已改由「真实 Win32 剪贴板往返单测 + 构建产物 + 代码审阅」覆盖，
      手工复现步骤见 `docs/UI_SPEC.md` §4.2 与 `docs/BENCH.md`。
      GUI 端到端「按键→首屏 ≤50 ms」仍待实机，不因本轮改动宣称完成。
      **（2026-11-15 同日已由 #78 的 GUI 真机轮补上，本节保留为当时的边界记录。）**

78. **GUI 真机验证（2026-11-15，第九轮）：Win32 真实输入 + WebView2 CDP 双通道**：
    - 起因：#77 记录浏览器原型验证受工具限制失败，用户要求「先做 GUI 真机验证」。
    - 构建：`cargo build --release --manifest-path app/src-tauri/Cargo.toml
      --features custom-protocol` → `sift-app.exe` 11 MB（E1）。启动时设
      `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`，
      CDP `/json/list` 拿到 `http://tauri.localhost/` 的 page target（E2）。
    - 通道：**Win32 真实鼠标/键盘**（`.work/gui_act.py`，纯 ctypes `SendInput` /
      `SetCursorPos` / `GetWindowRect` / `ClientToScreen`）负责操作；
      **CDP `Runtime.evaluate` / `Page.captureScreenshot`**（`.work/cdp.py`，
      纯标准库 WebSocket）负责 DOM 断言与截图。真实卷 C:/D:/E:/F:
      共 **2,179,900 条**索引，不依赖 MockEngine。
    - **P1-1 拖动** ✅：拖拽区（放大镜图标）真实按下并移动 (+200,+110)，
      窗口 rect `[260,100,1033,587]` → `[452,206,1225,693]`（+192,+106）（E6）；
      负向对照在输入框内拖动，rect 不变且仍能真实键入 `report`。
      `data-tauri-drag-region="deep"` 在 DOM 中确认（E3）。
    - **P0-2 复制路径** ✅：输入框无选区（`selectionStart===selectionEnd===6`）
      时真实 `Ctrl+C` → 系统剪贴板读回
      `D:\Sift\.worktrees\d\target\criterion\report`，状态栏 toast
      「已复制完整路径」（E7）。验证结束把原剪贴板内容 `403936` 写回。
    - **P0-3 右键菜单** ✅：真实右键点击结果行 → 菜单 DOM 四项
      （打开 / 打开所在文件夹 / 复制完整路径 / 复制文件名）+ 快捷键，
      rect `[301,133,216,139]` 完全落在 760×480 视口内（E8，
      图 `docs/evidence/gui-context-menu.png`）。
      `Esc` → `menu=false`、`input` 仍为 `report`、`IsWindowVisible=true`（E9）。
      纯键盘 `End` + `Enter` 选中「复制文件名」→ 剪贴板 `report` + toast
      「已复制文件名」（E10）；真实鼠标点第三项 → 完整路径 + toast。
      结果集 60s 过期后执行菜单项 → toast「请求不存在或已过期」（E13），
      失败透传（P1-9）也一并验证。
    - **首屏耗时（三轮真实按键）**：末次按键 → 首屏结果行渲染
      **145.6 / 146.2 / 146.8 ms**，引擎 `elapsed_ms` 6–7 ms（E11）。
      构成：`App.tsx` 的 `SEARCH_DEBOUNCE_MS = 120` 占主导，
      余下 ~26 ms 为 IPC + 引擎 + 渲染。**DESIGN §13-M3 的
      「每次按键到首屏 ≤50 ms」按端到端口径不达标**，且与 UI_SPEC §7.4
      明确要求的 120 ms 防抖在字面上互斥，见 #80。

79. **P1-1 的真正根因：Tauri ACL 未放行 `start_dragging`（2026-11-15 更正）**：
    - **更正上一轮口径**：#76 记「P1-1 已修复」并只做了拖拽区属性；真机首测
      **窗口纹丝不动**。`mousedown` 探针显示 `detail=1`、composedPath 含
      `DIV:deep`、`defaultPrevented=true`，证明 `tauri-2.12.5` 的 `drag.js`
      判定正确并调用了 IPC；但 `unhandledrejection` 抓到
      **`Command plugin:window|start_dragging not allowed by ACL`**（E4）。
    - 根因：项目**从来没有 capabilities 文件**，`app/src-tauri/gen/schemas/
      capabilities.json` 生成内容是 `{}`。前端其余调用都是
      `#[tauri::command]` 自定义命令（`search`/`range`/`status`/`open`/`hide`…）
      不走 ACL，所以此前一直没暴露；唯独 `plugin:window|start_dragging`
      属于 core 插件命令，必须显式授权。
    - 修复：新增 `app/src-tauri/capabilities/default.json`，`windows: ["main"]`、
      `permissions: ["core:window:allow-start-dragging"]`（最小权限，
      权限名核对自 `tauri-2.12.5/permissions/window/autogenerated/reference.md`）。
      重建后生成回显 `{"main-window":{...,"permissions":["core:window:allow-start-dragging"]}}`（E5），
      重测拖动成功、ACL 错误数组为空（E6）。
    - `gen/schemas` 已在 `app/src-tauri/.gitignore` 忽略，构建时自动再生成，
      不入库；**新 capability 文件必须入库**。
    - 结论：`data-tauri-drag-region` 是必要条件，capability 是充分条件，
      两者缺一不可。以后遇到「DOM 属性在、行为没发生」的 Tauri 功能，
      先查 `unhandledrejection` 再查 capability。

80. **真机新发现（未修，进 backlog，2026-11-15）**：
    - **P1-8 时间字段单位是 FILETIME，前端按 JS epoch 毫秒用**（E12）：
      CDP 直接 `invoke('status')` 拿到 `last_saved = 134392046912540464`（18 位，
      1601 纪元 100 ns），`new Date(...)` → **Invalid Date**；
      `types.ts` 里 `ResultRow.mtime` 注释写着「JS epoch 毫秒（M3 IPC 边界再
      转 FILETIME）」，但 `inprocess.rs:950` 直接 `mtime: index.mtime[eid]`
      把 FILETIME 原样送出。真机后果：修改时间列全是
      `NaN-NaN-NaN NaN:NaN`、状态栏右侧「已保存 NaN:NaN」
      （图 `docs/evidence/gui-time-format-bug.png`）。
      修复方向：IPC 边界统一转 epoch 毫秒（或前端换算），`formatTime` /
      `formatHHMM` 对 Invalid Date 也应有兜底。**本轮未改**，涉及引擎契约与
      M1/M2 数据口径，单独立项。
    - **P2 目录大小显示 `0 B`**：`ResultArea.tsx::RowContent` 无条件
      `formatSize(row.size)`，UI_SPEC §4.2 要求「目录显示空白」。截图可见
      所有目录行都显示 `0 B`。改动小，与上一项一并处理。
    - **P1-9 首屏目标与防抖冲突**：DESIGN §13-M3 验收写「≤50 ms」，
      `App.tsx` 的 120 ms 防抖是 M3 后按用户实机反馈「连续输入卡顿」加的
      （UI_SPEC §7.4）。实测 146 ms 端到端，其中引擎仅 6–7 ms。
      **二选一**：把验收口径改为「防抖后 ≤50 ms」（实测 ~26 ms 达标），
      或压低防抖值并重测卡顿。规范文字需同步，不改代码前先标注冲突。
    - **仍未覆盖**：托盘菜单点击、全局热键 `Ctrl+Alt+Space` 物理触发、
      失焦自动隐藏稳定性、窗口 hide/show 循环、中文 IME 组词。
      这些不是本轮三项，保持 #77/#80 的边界声明。
    - **2026-11-15 当天后续**：上面 P1-8 与目录 `0 B` 两项已由下一轮修复，
      见 **#81**；本条的 P1-9 首屏口径冲突与「仍未覆盖」项保持开放。
      本段保留为发现时的原始记录，便于追溯。

81. **P1-8：FILETIME → JS epoch 毫秒在 Rust IPC 边界转换（2026-11-15 修复）**：
    - **裁定「必须在 Rust 侧转」**：#80 留了「IPC 边界转 epoch 毫秒（或前端换算）」
      两个选项。本轮取前者，理由是定量的：典型 FILETIME ≈ 1.34e17，
      超过 IEEE-754 的 2^53 ≈ 9.01e15；JSON 反序列化到 JS 时精度已经丢失，
      `new Date(1.34e17)` 更是 `Invalid Date`，前端补算无解。反向换算出的
      epoch 毫秒最大约 9.22e14（`i64::MAX` ticks / 1e4），始终 < 2^53，
      JSON number 可无损表示。
    - **不动引擎契约**：`ff-engine::ResultRow.mtime` / `EngineStatus.last_saved`
      保持 FILETIME，`dm:` 求值、`SortKey::Modified` 排序、`.ffix` 持久化与
      `ff-cli` 全部不受影响。只新增 `app/src-tauri/src/timeconv.rs`，
      在 `range` / `status` 两个 Tauri 命令**序列化前就地转换**（字段名与类型
      不变，serde wire 契约冻结）。`api.rs` / `types.ts` 注释同步澄清两侧单位，
      并顺手修正 `name_matches` 注释里「UTF-8 字节区间、M6 再转」的过时口径
      （实际 `resolve_row` 已用 `to_utf16_ranges` 转成 UTF-16 码元）。
    - **边界规则**（`filetime_to_epoch_ms`，`app/src-tauri` 新增 7 个单测）：
      `ft <= 0`（NTFS「时间未设置」/数据损坏）→ `None`；换算后早于 1900-01-01
      或晚于 9999-12-31 → `None`（避免显示 `1601-1-1` 或公元 31000 年）；
      1900–1970 的真实老文件仍正常显示；亚毫秒按整除丢弃（UI 最细到秒）；
      `None` 语义不变——`stale_meta` 的 `None` 不得被写成 0。
      用 `i128` 做中间运算，将来入参改为 `u64` 也不会溢出。
    - **前端防御**：`format.ts` 新增 `toDate()`，对 `null` / `NaN` /
      `Infinity` / 越界值统一降级为空白（时间列）或 `—`（状态栏），杜绝再次把
      `NaN-NaN-NaN` 渲染到界面；`formatSize` 补有限性与负数检查；
      `ResultArea.tsx` 按 `is_dir` 传 `null`，目录大小列按 UI_SPEC §4.2 留白。
      `app/scripts/check-format.mjs` 用既有 `typescript` 转译单文件后跑
      **29 条断言**（`npm run test:format`），零新增依赖、无前端测试框架负担。
    - **验证**：`cargo test`（`app/src-tauri`）**7 passed** / 根 workspace
      **211 passed** / `npm run test:format` 29 assertions / `npm run build`
      50 modules / 双 workspace fmt + clippy `-D warnings` 全绿；
      Tauri release 真机 CDP 回显 `status.last_saved = 1794733215003`
      （13 位、有效 Date）、`range` 抽样 `invalidDates=0`、DOM 时间列
      `10-12 18:46` 等、状态栏「已保存 17:00」、目录大小空白而同屏文件行
      正常显示 KB、`anyNaN=false`；首屏 149.6 / 144.9 / 143.2 ms 与修复前
      145.6 / 146.2 / 146.8 ms 同量级（无性能回归）。
      证据图 `docs/evidence/gui-time-fixed.png`（与 `gui-time-format-bug.png`
      同为 `report` 场景，可直接对照）。
    - **自我评估 / 后续隐患**：
      ① `timeconv` 当前只在 Tauri 外壳。**M6 服务化把 `EngineClient` 暴露成
      HTTP 时必须复用同一转换**（可上移到 `ff-engine` 公共模块），否则新通道
      会重现同一个 bug；
      ② 1900 年前的**真实**文件会显示空白——按「时间未设置远多于百年老文件」
      取舍，如需放宽只改 `MIN_VALID_EPOCH_MS` 一个常数；
      ③ 转换精度 1 ms，未来 UI 若要显示亚秒需改契约，不能只改前端；
      ④ 真机交互桌面存在外部输入/剪贴板串扰（一次 `ctrl+a` + `type` 连续
      发送时把外部文本拼进了输入框），已改为分步键入 + CDP 直调 IPC 复核，
      结论不受影响；这类噪声下不要再依赖单次复合键盘脚本的回显；
      ⑤ #80 的首屏 50 ms 口径冲突、托盘 / 热键 / IME 未覆盖项仍开放，本轮不碰。

82. **首屏 50 ms 口径裁定（2026-11-15，P1-9 关闭）**：
    - **结论**：恢复 #66 的原始定义并写进验收文字——**「≤50 ms」指检索触发后的
      执行段（引擎 + IPC + 首屏渲染），不含输入停顿窗口**；从按键算起的端到端
      感知延迟（实测 145–150 ms）作为**独立体验指标**保留记录，不删除、不改写，
      也不再判为验收失败。`DESIGN` §13-M3 明确「典型查询 = ≥3 字符子串、
      命中数 ≤10 万」。**保留 120 ms 防抖，不改搜索行为。**
    - **为什么不是「改数字迁就实现」**：#80 把冲突表述成「二选一」（改口径 /
      压低防抖），隐含前提是 50 ms 原本指端到端。回查 #66 原文，加防抖当轮
      已写明「DESIGN §13-M3 的『按键→首屏 ≤50ms』指**引擎检索延迟**，防抖是
      叠加其上的 120ms 输入停顿窗口……不是对该指标的破坏」；`UI_SPEC` §3.3 /
      `DESIGN` §11.1 的「不防抖」是 M3 原设计，实现偏离时未同步文字，才在
      #78/#80 被按端到端口径重测并误判。**这是文档口径漂移，不是性能退化。**
    - **排除「压低/去掉防抖」的实测依据**（release `ff-cli`，真实 C: 卷
      817,903 条，`--max 5`，单次）：

      | 查询 | `r` | `a` | `re` | `rep` | `repo` | `report` |
      |---|---:|---:|---:|---:|---:|---:|
      | 搜索耗时 | **283 ms** | **443 ms** | 93 ms | 7 ms | 4 ms | 4 ms |

      单字符命中数百万条 → `search_ranked` 对全部命中评分排序（`Top-K 部分选择`
      未做，#71）。若首字符零防抖或把窗口压到 50 ms 以下，连续输入 `report` 会
      依次触发 283 ms 级全表检索 6 次，#66 修掉的卡顿会**成倍复发**。
      因此「让端到端也 ≤50 ms」在当前引擎能力下不成立，除非先做 Top-K 部分
      选择（属 M5 遗留，另行立项）。
    - **分段数据**：端到端 145–150 ms ≈ 120 ms 防抖 + ~26 ms 执行段；
      执行段中引擎 `elapsed_ms` 6–12 ms（真实卷 2,182,600 条，`report` 命中
      2,898 条），余下为 IPC 往返 + React 虚拟列表首屏渲染。按 #66 口径达标。
    - **同步改动**：`DESIGN` §11.1 / §13-M3 验收与状态块、`UI_SPEC` §3.3 /
      §4.1 / §7.4（补防抖与状态机关系、更正「防抖被 DESIGN 禁止」的过时表述、
      说明 250 ms spinner 阈值在 120 ms 防抖下不触发属预期）、`App.tsx` 防抖常量
      处加实测数据警示防回归、`BENCH` 新增裁定实测节、`REVIEW_BRIEF` 抬头与 §5。
    - **仍开放（不因本条关闭）**：① 单/双字符查询 93–443 ms 的体验优化——
      正解是 `Top-K 部分选择`（#71）或短查询用 T1 提前返回，需改引擎并回归
      金标准用例；② 端到端 145–150 ms 的感知延迟可通过「等待期即时视觉反馈」
      缓解（当前靠输入立即回显 + 保留旧结果承担，未加骨架/进度）；
      ③ 托盘 / 全局热键 / 失焦隐藏 / IME 组词的实机覆盖。

83. **动效与质感收尾：补齐窗口动效缺口 + 轻微 spring 手感（2026-11-15，进 M6 前）**：
    - **动机**：用户反馈「质感与动效没到位」，并点名 ZTools / CompressO 两个
      参考项目可借鉴。核对规范后判定：灰阶配色 / Segoe UI / 零装饰是 `UI_SPEC`
      §1 / §2.3.1 的**刻意取向**（「无个性的界面」），不是实现缺陷；真实缺口是
      §2.6 的 `dur-window-in` / `dur-window-out` / `ease-in` 三个 token
      **定义了但 CSS 引用 0 次**——窗口唤出/隐藏动效规范要求了却从未实现。
      用户拍板：补缺口 + 轻微质感动效，不动色板与字体取向，完成后进 M6。
    - **窗口动效实现**：原生 `hide()` 是瞬时的，直接调会把退出动画吞掉。方案：
      `show_window` 在 show 后向前端 `emit("sift:shown")`；`hide_window` 先
      `emit("sift:hiding")`，再起线程等 90ms（`WINDOW_OUT_MS` = §2.6
      `dur-window-out`）后真隐藏。**竞态**：90ms 窗口内热键连按 toggle 会
      先 show 后 hide——用 `HIDE_GEN: AtomicU64` 代数计数：hide 调度时记下
      代数，到点后代数一致才真隐藏；show 递增代数使挂起隐藏全部作废。
      失焦自动隐藏路径（`on_window_event`）改为统一走 `hide_window`。
      前端 `App.tsx` 监听两事件直接切换 `.app-window` 的动画 class
      （**不经 React 状态**：动画类与渲染无关，避免整树重渲染）；
      `window-in` 用 `backwards` fill（结束回自然态，不长期保留 transform
      建 containing block）、`window-out` 用 `forwards`（真隐藏前保持透明）。
      浏览器原型无 Tauri 事件，跳过（原型 `hidden` state 显隐不受影响）。
    - **轻微 spring 手感**：新增 `--ease-spring: cubic-bezier(0.2, 1.08, 0.3, 1)`
      （进入带 ~0.4% 过冲），手感参考 CompressO 的 Framer Motion spring 与
      ZTools 的移动端交互；**只用于窗口唤出与右键菜单出现**两处，其余状态
      过渡仍用 `ease-out`，`§2.6`「动效克制」取向不变。**未引入** framer-motion
      （零新增依赖，见 DEPENDENCIES）；ZTools 的「离屏首帧就绪守卫」（show 前
      预热、首帧完成才显示）记入 backlog，Tauri 下可用同思路消除呼出白屏。
    - **其余质感项**：右键菜单出现 120ms `scale .96→1` + fade（原点跟随鼠标侧，
      `transform-origin: top left`）；关闭即卸载**不做退出动画**（90ms 退出会挡
      下一步点击，收益为负）。列表行 hover / 选中底色 160ms 过渡（§2.6
      `dur-base` 本就规定）。`<mark>` 高亮**不做**出现动画：虚拟滚动下每屏
      都会触发，干扰滚动性能。
    - **顺带修复真机 bug**：`App.tsx` Esc 隐藏窗口调 `invoke('hide')`，而
      `generate_handler!` 注册的命令名是 `hide_window`——真机 Esc 隐藏
      **静默失效**（Promise rejection 被 `void` 吞掉）。上一轮 P0-3 验证时
      Esc 是在菜单打开场景测的（只关菜单），菜单关闭后的 Esc 隐藏从未实测，
      故一直没暴露。改为 `invoke('hide_window')`。
    - **验证（真机 release + CDP，2026-11-15）**：布局 `display:flex`、
      状态栏贴底（gap 1px）、`scrollW 760`、虚拟化正常（`report` 2,899 条
      仅 23 个行节点）、搜索 8 ms；`sift:hiding` → `.window-exit`
      （`window-out` 播放中 opacity 中间帧实测 0.84）→ 90ms 后真隐藏
      （日志 `hide result ok=true`）；`sift:shown` → `.window-enter` 重播、
      opacity 恢复 1；**hide 后 30ms 内 show → 挂起隐藏作废**（`gen_match=false`）、
      窗口不被误藏；**Esc（空输入）真机隐藏生效**（`visible=false`）；
      右键菜单 `menu-in` 120ms、`transform-origin 0 0`、行过渡 160ms。
      失焦自动隐藏在真实焦点抖动（fg_pid=0/终端抢焦点）下多次正确触发。
      证据图 `docs/evidence/gui-menu-anim.png`。
    - **本轮连续踩掉三个坑（有日志/回显佐证，过程留档）**：
      ① **ACL 二次坑（同 #79 模式）**：`__TAURI__.event.listen` 走
      `plugin:event|listen`，被 ACL 拒绝（`unhandledrejection` 抓到），
      capabilities 补 `core:event:allow-listen` + `allow-unlisten`（最小权限，
      核对自 tauri-2.12.5 permissions/event/autogenerated/reference.md）。
      结论与 #79 一致：**凡新增前端 → core plugin 的调用，先过一遍 ACL**。
      ② **`fetch_add` 语义错误**：延迟隐藏线程用 `load() == fetch_add 返回值`
      作 gen 校验——`fetch_add` 返回**旧值**且自身已 +1，校验**恒 false**，
      每次延迟隐藏 100% 被自己作废（日志 `gen_match=false (gen=N, now=N+1)`
      三连，且 `hide result` 行从未出现）。改为 `load() == gen + 1` 后修复。
      ③ **Edit 吞块（用户报「UI 变异常」的真凶）**：给 `.app-window` 加
      `will-change` 的一次 Edit 在 new_string 里多写了一个 `}`，把 CSS 块
      **提前闭合**，`display:flex; flex-direction:column; font-size:13px`
      三行被挤出选择器变成无效规则——flex 布局塌成 block：状态栏悬在中部、
      虚拟列表容器测高异常**全量渲染 2,899 行**（resultarea 高 9 万 px）、
      内容整体错位透底。`will-change` 只是帮凶（合成层副作用又叠加了一次
      滚动条溢出，`scrollW 752/bodyW 745`），真凶是块损坏。修复：恢复三行 +
      删除 will-change + 在 CSS 块内留防回归注释。
      **教训**：① 用户报「界面变异常」时先 `git diff` 比对结构文件，
      不要只在新增逻辑里找；② 编辑结构化块（CSS 规则/JSON）时 old/new_string
      必须带全块边界，收尾大括号配对要数；③ `fetch_add/swap/load` 语义差异
      在注释里写死，防再次误用。
    - **边界**：hide 延迟的 90ms 内窗口处于退出动画中（可见但透明化），期间
      失焦再次触发 hide 会重复 emit（幂等，两次调度只后代数生效）；
      `toggle_window` 的 visible 判定在 90ms 窗口内可能读到「仍可见」→
      走 hide 分支重复调度（同样幂等）。`prefers-reduced-motion` 下动画
      0.01ms，Rust 侧仍固定等 90ms 再隐藏——多等 90ms 无感知影响，不改。

84. **UI 重整替代「只做轻微质感收尾」计划**（2026-11-15，用户要求继续改）：
    - 用户反馈整体设计怪、质感差，旧规格不应限制组件替换；本轮同步修订
      `UI_SPEC` v0.2 与 `DESIGN`，保留紧凑搜索工具、无侧栏、双主题、键盘操作
      与虚拟化等工程约束，不进入 M6。
    - 搜索图标、输入、清空/进度合为一个 42px 搜索框，设置在框外；筛选降低
      填充强度，旁置原生排序下拉。列头只切换升/降序，返回相关度有明确入口。
    - 亮黄匹配底改为双主题低干扰蓝底，文字对比实算浅 8.33:1 / 深 7.04:1；
      使用既有 SVG 图标与类型色，不新增图标包、不冒充 Shell 图标已接线。
    - 索引完整长状态收进 `details/summary`，底栏只留短标签；Esc 局部收起，
      不清空查询。未接线设置真正禁用，选项集改下拉、管理/配置改命令按钮，
      高级项折叠；已接线三开关仍可用，持久化欠账不掩盖。
    - 原生根容器填满客户区，预览居中单独处理；窄窗口筛选换行、四列保留。
      清理 window-out 后残留无效声明/大括号，恢复 topbar 样式；固定设置页头，
      消除搜索框重复焦点环并给四列增加 8px 间距。
    - 保留 120ms 防抖、32px 虚拟行、FILETIME IPC 转换与 #83 动效/ACL；
      本轮没有修改后端代码，没有提交，也没有新应用依赖。
    - 新增外置 Playwright 回归 `app/scripts/check-ui.cjs`（`npm run test:ui`）；
      构建、29 项格式断言、双主题/排序/筛选/设置/Esc/窄窗口/错误/减弱动效
      浏览器检查通过。760×480 / 480×320 只做浏览器原生布局模拟，
      **真实 Tauri/WebView2 复验与用户视觉验收尚待完成**；旧 #83 真机结果
      不替代本轮验收。复现方式见 `BENCH`。

85. **借鉴 CompressO 的设置转场节奏，不照搬手机抽屉外观**（2026-11-15）：
    - 用户已认可 #84 并要求提交、进入 M6，基线提交为 `e536db0`。
    - 核对 `codeforreal1/compressO` 的 `src/components/Drawer/index.tsx`：使用
      Vaul（不是 Framer Motion），带 handle、圆角顶边、遮罩与背景缩放。
    - 用户随后强调适配与一致性；Sift 是 760×480 紧凑桌面工具，设置已经是
      全页视图，因此仅借鉴向上进入/向下退出的节奏，不引入 Vaul，不增加拖拽
      手势、手机式大圆角或缩放背景。独立实现 CSS，不复制 CompressO 源代码。
    - 设置 220ms/24px 向上淡入、150ms/16px 向下淡出，沿用现有曲线与
      reduced-motion；180ms 超时兜底。打开聚焦返回按钮，关闭保留查询并恢复
      输入焦点。其余布局、颜色、组件不变。

86. **M6 首批先落共享协议和时间边界，不直接把单客户端引擎套成服务**（2026-11-15）：
    - 用户确认进入 M6，#85 设置轻转场提交为 `b9075e9`。本批可验收目标是
      长度前缀 JSON 协议正确处理有效帧、分片/粘连帧、断线、坏帧和资源上限，
      同时让 Tauri/HTTP 复用同一 FILETIME 转换，不改变现有应用的运行形态。
    - `ff-engine::ipc` 为纯 Rust/serde 模块，无 OS 管道、无网络监听，接口消息
      与 EngineClient 方法对应；版本 1、非零传输 request_id，区别于搜索句柄 id。
      每帧 ≤1 MiB、查询 ≤16 KiB、range ≤1000 且校验 offset+limit 溢出；
      写端先有界编码，再写长度头，编码错误不会留下半帧。I/O/解析失败后应断开连接。
    - `Open` 响应只携带解析后的行与 action，未来服务不得调用 SYSTEM shell/剪贴板；
      Pipe 客户端仍由 UI 身份执行真实动作（DESIGN §12）。
    - `timeconv.rs` 从 Tauri 移到 ff-engine，原 7 项测试原样迁移；引擎与 Rust
      管道保持 FILETIME，最终面向 JS 的 Tauri/HTTP 边界转换，避免二次换算。
    - 直接依赖复用锁定 serde_json 1.0.154，已读本地源码核对序列化接口与
      MIT OR Apache-2.0 许可证；无新网络框架/运行时/服务依赖。
    - 先写 11 项 IPC 集成测试，缺模块/依赖时编译失败，再补实现后全部通过。
      根 workspace fmt/clippy 与 229 测试通过；其中包含迁移的 7 项时间测试。
      Tauri fmt/clippy/test/custom-protocol check 通过（本地测试 0 项，已迁移而非删除）。
    - **仍未实现**：多客户端会话隔离、SID ACL 管道与重连、ff-service/SCM 安装卸载、
      普通权限 UI、HTTP 开关/token/Host/rate-limit 与其验收测试；不标记 M6 完成。

87. **桌面版索引改放运行盘根目录并清理幽灵卷文件**（2026-11-16）：
    - `%LOCALAPPDATA%` 可能位于空间紧张的 C:，而索引体积随卷规模增长；桌面版改用
      运行程序所在盘的 `Sift\` 目录，服务模式仍按 DESIGN 使用 `%ProgramData%`。
    - 启动枚举当前卷后，仅保留有效 serial 对应的 `.ffix`，删除已卸载卷或旧序列号文件；
      同一卷仍使用固定文件名与原子覆盖，不会因重复索引产生副本。

88. **M6 第 1 项：Windows OS 命名管道与 ff-service 落地**（2026-11-16）：
    - 新增两个 workspace crate：`ff-ipc`（管道传输，仅 Windows 编译）与 `ff-service`
      （Windows 服务外壳）。协议类型/编解码仍只在 `ff-engine::ipc`，管道层不重复实现。
    - SID/ACL：管道名 `\.\pipe\sift-<用户 SID>`；SDDL `D:P(A;;GA;;;<用户SID>)(A;;GA;;;SY)`
      仅放行登录用户与 SYSTEM、禁止继承。服务以 LocalSystem 运行时用
      `WTSGetActiveConsoleSessionId` + `WTSQueryUserToken` 取活动控制台用户的 SID
      （不能用服务自身的 SYSTEM SID，否则 UI 被 ACL 挡住）；无交互会话时回退并拒绝启动管道。
    - 传输：`PIPE_TYPE_BYTE|PIPE_READMODE_BYTE|PIPE_WAIT` + 有界实例；`HANDLE` 封装成
      `Read/Write` 复用 `serve_connection`；每连接 `create_session()` + 独立线程，
      支持多客户端并发、坏帧只丢该连接。刻意不调用 `DisconnectNamedPipe`：每个 accept
      关闭的是独立句柄实例，让内核回收更安全。
    - `EngineClient over Pipe`（`PipeClient`）经管道调 Search/Range/Cancel/Status/Rescan/Open；
      `Open` 只解析出 `ResolvedAction` 存到 `last_open()` 供 UI 以自身身份执行（DESIGN §12），
      服务绝不代替用户打开文件/写剪贴板。
    - `ff-service` 支持 `install/uninstall/start/stop/status` SCM 命令（需管理员）；
      服务模式索引放 `%ProgramData%\Sift`；control handler 收 Stop/Shutdown 置停止旗标、
      占位连接唤醒 accept、保存索引后上报 Stopped。
    - 新依赖 `windows-service 0.8.1`（MIT/Apache-2.0，§2.1 清单内）与 windows 新增
      `Win32_System_Pipes`/`Win32_Security*`/`Win32_System_RemoteDesktop` features，
      已登记 DEPENDENCIES.md；windows-sys 0.61 与本地已有版本一致，无冲突。
    - 验证：workspace fmt/clippy(-D warnings)/test 全绿；新增 4 项 SID/SDDL 单测 +
      3 项真实 OS 管道端到端测试（往返、多客户端会话隔离、卷 ID 解析）通过；
      `ff-service status/bogus` 命令路径实机冒烟通过。**未实测**：SCM 实际安装/启停
      （需管理员、会改系统服务）、SYSTEM 权限下跨会话 ACL、服务崩溃重连、HTTP（未开始）。

89. **M6 普通权限 Tauri 接 PipeClient + 断线重连**（2026-11-16）：
    - Tauri `EngineState` 改为 `Arc<Backend>`：`InProcess`（管理员兜底）与 `Pipe`
      （`ff_ipc::PipeClient`）。启动时按当前用户 SID 构造管道名并尝试连接，成功用
      Pipe、失败回退 InProcess——未装 SCM 的机器行为与 M0–M5 完全一致；六个 Tauri
      命令签名与前端契约不变。
    - `open` 双路语义：InProcess 由引擎内部执行动作；Pipe 由服务经 IPC 只返回
      `ResolvedAction`，UI 进程调新增的 `ff_engine::execute_open_checked` 以当前用户
      身份执行（DESIGN §12，服务绝不以 SYSTEM 代用户打开/写剪贴板）。新增
      `ff-engine::actions` 模块（3 项纯函数单测），把 inprocess 已有的 Win32 FFI 暴露给 UI 层。
    - 重连：`PipeBackend::with_reconnect` 检测传输错误（`EngineError::Io` 含 `pipe`），
      1s 退避后重建 PipeClient 并至多重试一次；`status_checked` 使状态轮询也走重连保护。
      结果经 Tauri event `sift:service-status` 发「服务已重连 / 服务已断开，正在重试」，
      前端 `App.tsx` 用状态栏 toast 如实提示。退避上限避免服务未起时忙重连。
    - 已知限制：重连后服务侧 `EngineSession` 为全新实例，旧 `request_id` 失效需重新
      搜索（toast 已提示）；SCM 安装/启停、SYSTEM 跨会话 ACL 真机实测仍未做。
    - 验证：workspace + Tauri 的 fmt/clippy(-D warnings)/test 全绿；前端 build + format
      通过；Pipe 路径由 ff-ipc 3 项真实 OS 管道端到端测试覆盖。
