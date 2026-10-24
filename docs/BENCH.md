# BENCH.md —— 实测数据（DESIGN §14）

> 性能数字必须实测；没测过的不要写成事实。格式：日期 · 机器 · 条目数 · 指标 · 数值。
> 合成数据基准由 Track D 记录；真实卷 M0 数据由 Orchestrator 合并时追加。

## 基准机器

- OS：Windows 10 专业工作站版（x64）
- CPU：AMD Ryzen 5 5600U（6 核 / 12 线程）
- 内存：15.3 GB
- 磁盘：HP SSD EX900 1TB（固定盘）+ WD P40 Game Drive 1TB（外置）；工作区位于 D: 卷
- 采集日：2026-10-24（Orchestrator 采集，Track D 可补充磁盘型号细目）

## M0 · 真实卷摸底（Track A，真实 NTFS 卷）

> 原始明细见 `crates/ff-ntfs/NOTES.md`。以下为一次实机摸底（release、管理员权限、单次实测），不是多次平均或稳定性能承诺。

- 机器：Windows 10 专业工作站版 x64；AMD Ryzen 5 5600U（6 核 / 12 线程）；15.3 GB 内存；C: 为固定 NTFS 卷（非工作区所在盘，工作区在 D:）。
- 运行：`ff-cli scan C: ntdll`（先 `FSCTL_QUERY_USN_JOURNAL` 记录 journal_id/next_usn，再两遍读 `$MFT`）。
- 卷序列号：`0x9EA0C151`；扫描前 USN：Journal ID `133932538930439016`，Next USN `24804370080`。

| 条目数 | 指标 | 实测值 |
|---|---:|---:|
| 849,366 | 全量耗时（含 USN 查询 + MFT 两遍 + 写入 `Vec<RawEntry>`，不含卷枚举/搜索） | 5.312 s |
| 849,366 | 父解析警告（挂到根下的条目） | 0 |
| 849,366 | 扫描后峰值 Working Set | 142.66 MiB（149,585,920 B） |
| 849,366 | 扫描后当前 Working Set | 106.86 MiB（112,054,272 B） |
| — | 朴素子串 `ntdll`（逐条 `to_lowercase().contains`，命中 60 条） | 88.022 ms |

> 说明：M0 朴素搜索未走后续 memchr/rayon 快速路径，仅作基线对照；真实索引建立耗时以 M0 的 5.312 s 为准，合成 200 万档的 1.08 s 为不同（合成、SoA、流式）口径，二者不可直接比较。

## M1 · 合成数据（Track D）

### 数据与测量方法

- 日期：2026-10-24；机器：同上（Ryzen 5 5600U / 15.3 GB / D: SSD）。
- 配置：`count=2,000,000`，固定种子 `0x5349_4654`，`max_depth=8`，`depth_decay=0.55`，`dir_ratio=0.15`，`cjk_ratio=0.25`，`dup_ratio=0.12`，`compound_ratio=0.10`；确定性生成器流式喂给 `IndexBuilder`，不驻留合成条目 `Vec`。
- 工具：Windows x64 release 构建；`cargo run --release -p sift-bench --bin memprobe -- 2000000`。建索引与全量 `write_path` 为单次实测，搜索为 7 次实测取中位数；内存来自 `GetProcessMemoryInfo`，是整个 `memprobe` 进程的 Working Set / Peak Working Set，不是仅索引堆分配量。
- 下表全部为 **200 万条实测**（另加 1 个虚拟根，索引共 2,000,001 条），无外推；`unresolved_parents=0`。

| 条目数 | 指标 | 实测值 | DESIGN §1.3 目标 |
|---|---|---:|---:|
| 2,000,000 | 建索引（含确定性数据生成） | 1,112.4 ms | ≤ 30 s，通过 |
| 2,000,000 | 子串 `report`（78,607 命中，7 次中位） | 2.5 ms | ≤ 100 ms，通过 |
| 2,000,000 | 子串稀有完整文件名（1 命中，7 次中位） | 1.0 ms | ≤ 100 ms，通过 |
| 2,000,000 | AND `report summary`（1,609 命中，7 次中位） | 3.2 ms | — |
| 2,000,000 | `write_path` 全量 2,000,001 条（单次） | 278.0 ms | — |
| 2,000,000 | 进程 Working Set | 238.5 MB | ≤ 300 MB，通过 |
| 2,000,000 | 进程 Peak Working Set | 255.5 MB | ≤ 300 MB，通过 |

> 以上数字是合成索引微基准，不等同于真实 NTFS 卷扫描；真实卷建索引时间由 Track A 的 M0 摸底记录。

### Criterion 分档结果

复现命令：`cargo bench -p sift-bench --bench build_index` 与 `cargo bench -p sift-bench --bench search_bench`。下列均为 **2026-10-24 同机实测的 Criterion 中央估计值**，不是由 200 万档外推；建索引每档 20/20/10 个样本，搜索与路径每档 100 个样本。

| 基准 | 条目数 | 实测中央估计 |
|---|---:|---:|
| 建索引 | 10,000 | 4.001 ms |
| 建索引 | 100,000 | 49.906 ms |
| 建索引 | 1,000,000 | 619.03 ms |
| 子串 `report` | 100,000 | 174.98 µs |
| 子串 `report` | 1,000,000 | 1.3159 ms |
| 稀有完整文件名（1 命中） | 100,000 | 24.446 µs |
| 稀有完整文件名（1 命中） | 1,000,000 | 175.07 µs |
| AND `report summary` | 100,000 | 316.63 µs |
| AND `report summary` | 1,000,000 | 1.5136 ms |
| `write_path` 均匀采样 9,901 条 | 1,000,000 | 1.4537 ms |

200 万档由上面的 `memprobe` 直接实测，因此没有使用小档外推值。

## 第二轮 · 真实卷端到端（Track E + Track F，真实 NTFS 卷）

> 2026-10-24 同机**单次实测**（release、管理员权限），取自 `ff-cli search` 端到端跑通的输出；不是多次平均或稳定性能承诺。

- 机器：同「基准机器」（Ryzen 5 5600U / 15.3 GB / C: 固定 NTFS 卷，工作区仍在 D:）。
- 运行：`ff-cli search C: <查询> --max N`（枚举 NTFS 卷 → `InProcessEngine::rescan(Some(卷))` 流式全量扫 MFT 并建 SoA 索引 → `search` → `range` 取窗口行）。
- 卷序列号：`0x9EA0C151`。

| 条目数 | 指标 | 实测值 |
|---|---:|---:|
| 878,732 | 建索引（真实全量扫描 + SoA 索引构建，`rescan` 计时） | 4.803 s |
| 878,732 | 子串 `ntdll`（`--max 5`，`search` 内部计时） | 1 ms |
| 878,729 | 多词 AND `system dll`（`--sort size --max 3`） | 3 ms |

> **条目数在两次运行间有微小差异（878,732 / 878,729）是活卷的正常现象**——两次全量扫描之间有条目被创建或删除。这正是 M0 记录的「活卷无快照一致性」风险点，由 M2 的 USN 增量（从扫描前记录的 `next_usn` 续读）补齐。
>
> **口径说明**：M0 在同一真实卷上的朴素子串 `ntdll` 基线为 88.022 ms，走 SoA 索引后 `search` 为 1 ms。二者口径不同（朴素为逐条 `to_lowercase().contains`，含大量临时分配；索引走 memchr/rayon 并行 `memmem` 且结果已是 id），数量级差异可作参照，但不应作为严格的加速比。
>
> **未采集项**：本轮未测量端到端峰值内存（留给后续实机复核）。合成 2M 的内存实测（238.5 MB / 峰值 255.5 MB）见上节。
