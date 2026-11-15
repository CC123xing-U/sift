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

## M2 · 持久化 + 增量（feat/m2-persistence，2026-10-24）

> **口径**：M2 的真实卷端到端项（真实 journal 增量延迟、重启载入计时、强制 kill 演练）需管理员 + 真实 NTFS 卷，**不进 CI**（决策 26）。以下为 CI 单测覆盖 + 设计参数，真实计时待 M2 收尾实机复核（见 DESIGN §13-M2「状态」）。

### CI 单测覆盖（ff-engine，`cargo test -p ff-engine`，19 个单测）

| 验证点 | 单测 | 覆盖内容 |
|---|---|---|
| Overlay upsert 反映到搜索 | `overlay_upsert_reflected_in_search` | `FakeScanner` 注入 upsert → `search` 命中新增条目 |
| Overlay remove 从搜索排除 | `overlay_remove_excluded_from_search` | `FakeScanner` 注入 delete → `search` 不再命中 |
| journal 失效触发全量重建 | `journal_unavailable_triggers_rescan` | `FakeScanner.unavailable=true` → `JournalUnavailable` → `rebuild_volume` 重建索引且结果一致 |
| 持久化 roundtrip | `engine_persistence_roundtrip` | `save_all` 写 `<serial>.ffix` → `load_all` 读回 → `query` 命中一致 |

### 设计参数（落地值，非实测性能）

| 参数 | 值 | 说明 |
|---|---:|---|
| `MERGE_THRESHOLD` | 50,000 | Overlay 条目数超此阈值触发 `merge_overlay_into_base` |
| 后台轮询 slice | 100 ms | `background_loop` 每轮 `poll_all → maybe_save → reap_stale` |
| `SAVE_INTERVAL` | 10 s | 落盘节流，避免每次变更都写盘 |
| `RESULT_TTL` | 60 s | 命中集无访问回收（落实决策 54，#54 缺口关闭） |
| FFIX 容错 | 长度/魔数/CRC 校验 | 损坏文件 `load` 返回 `None` → 上层重建，**不崩溃** |

### 待实机复核（不在 CI）

- 重启载入 ≤ 0.5s：设计目标，真实大卷计时待补。
- 被索引卷 create/rename/move/delete 后 ≤ 1s 搜索结果正确：单测覆盖逻辑，真实 journal 端到端需管理员实机。
- 保存中强制结束进程仍能载入：self-CRC32 损坏检测 + 降级重建设计已落地，CI 有容错 `load` 路径（损坏→`None`），实机 kill 演练待补。
- 合并期间持续搜索 p99 延迟：merger 在阈值触发、同步但边界有限，p99 待实机测。

---

## M3 · 可用 UI（Tauri 外壳 + InProcess 引擎，2026-10-24）

### 编译链验证（沙箱内实测，exit code 0）

| 门禁 | 范围 | 结果 |
|---|---|---|
| `cargo fmt --all -- --check` | 根 workspace + `app/src-tauri` | 绿 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 根 workspace | 绿 |
| `cargo clippy --all-targets -- -D warnings` | `app/src-tauri`（独立 workspace） | 绿 |
| `cargo test --workspace` | 根 workspace（ff-engine 19 单测） | 绿 |
| `cargo check` | `app/src-tauri`（tauri v2.12.5 全依赖树） | 绿 |
| `cargo build` | `app/src-tauri` → `target/debug/sift-app.exe` 15.2 MB（debug，完整链接） | 绿 |
| `npm run build` | `app/`（tsc -b + vite build，50 modules） | 绿 |

### IPC 契约形态（前端 TS ↔ Rust，冻结契约保持 Rust 类型不变）

| 契约字段 | Rust 类型 | 线上形态 | TS 类型 |
|---|---|---|---|
| `ParseError.span` | `Range<usize>` | `{start, end}` | `{start:number; end:number}` |
| `ResultRow.name_matches` | `Vec<Range<u32>>` | `[[start,end],...]` | `[number, number][]` |
| 枚举（`SortKey`/`OpenAction`/`VolumeState`） | Rust enum | snake_case 字符串 | 字面量联合 |
| `VolumeStatus.volume` | `String`（IPC 边界由 `VolumeId` 降维） | string | `string` |

### 待实机复核（不在 CI，需 WebView2 运行时 + 显示器）

- 按键 → 首屏结果 ≤ 50ms（典型查询）：**2026-11-15 真机实测 145.6 / 146.2 / 146.8 ms，端到端口径不达标**；防抖后段（搜索→渲染）约 26 ms 达标。引擎 6–7 ms。详见「GUI 真机验证」节与 DECISIONS #78/#80。
- 窗口隐藏 / 唤出稳定性（失焦自动隐藏 + 全局热键唤出）：代码路径已落地，行为验证待实机。
- 托盘菜单（显示/隐藏/重建索引/退出）实际交互：待实机。
- `Ctrl+Alt+Space` 全局快捷键注册与 `toggle_window`：待实机。
- 双击 Ctrl 唤出（`WH_KEYBOARD_LL`）：**未实现**，留待后续（设置项可关）。

---

## M4 · 查询语法与属性过滤（2026-10-26）

### 真实卷实测（C: 卷，880,465 条，release + 管理员，单次）

复现命令：`cargo build --release -p ff-cli && ./target/release/ff-cli.exe search C: "<查询>" --max 3`
（同一台基准机器；`ff-cli` 每次先全量建索引约 3.8–4.3 s，下表为**搜索耗时**）。

| 查询 | 走的路径 | 搜索耗时 |
|---|---|---:|
| `folder:Windows` | 子串 + 属性过滤（memmem 驱动） | **3 ms** |
| `ext:pdf` | 字面探针剪枝后验证 | **2 ms**（剪枝前 67 ms） |
| `*.log` | 通配符，字面探针剪枝 | **5 ms**（剪枝前 67 ms） |
| `ext:doc;docx;pdf;md;txt;rtf;xls;xlsx;csv;ppt;pptx`（UI「文档」chip） | 11 个探针并集 | **33 ms**（剪枝前 76 ms） |
| `ext:jpg;jpeg;png;gif;bmp;webp;svg;ico;tif;tiff`（UI「图片」chip） | 10 个探针并集 | **46 ms** |
| `size:>500mb` | SoA 列全表扫描 | **6 ms** |
| `regex:^\w+\.log$` | 正则，无探针可剪枝 → 全表 | **69 ms** |

**结论**：`ext:` / 通配符经「字面探针剪枝」后从 67 ms 降到 2–5 ms；多扩展名（UI chips）降到 33–46 ms；
正则与路径类仍走全表（69 ms）。全部落在 DESIGN §1.3 的「子串搜索 ≤ 100 ms」目标区间内。

### 语法测试集

`crates/ff-core/tests/m4_syntax.rs`：**106 条**表驱动用例（验收要求 ≥ 80），
覆盖合法语法、错误输入、半成品输入、每个修饰符的边界、NOT / OR 优先级，
并额外断言「结果恒升序去重」与「取消不返回半成品」。

### 门禁

`cargo fmt` / `cargo clippy --workspace --all-targets -D warnings`（双 workspace 含 `app/src-tauri`）/
`cargo test --workspace`（**179 passed**）/ `npm run build` 全绿。

## M5 · 模糊 + 拼音 + 评分（2026-11-15）

**基准机器**：同 §6（Windows 10 专业工作站版 x64 / Ryzen 5 5600U 6C12T / 15.3 GB）。
**数据**：`sift_bench::build_index(&SynthConfig::with_count(2_000_000))`，
固定种子 `0x5349_4654`，2,000,001 条（含虚拟根），其中 `cjk_ids` 591,778 条，
建索引 1.067 s。**release** 构建，每项跑 3 次取最小值。

**复现**：

```bash
export TMP=/f/Temp TMPDIR=/f/Temp TEMP=/f/Temp
cargo run --release -p sift-bench --bin m5probe
# 或 criterion 组（10 万 / 100 万档，拼音 / 模糊 / 评分排序）：
cargo bench -p sift-bench --bench search_bench -- m5
```

### 终态实测（DESIGN §8.1 门槛：200 万条下模糊全表 ≤ 150 ms）

| 场景 | 命中数 | 实测 | 判定 |
|---|---:|---:|---|
| T1 子串 `report` | 78,607 | **2.5 ms** | ✅ |
| T2 拼音 `zgr`（遍历 591,778 CJK 条目，0 命中＝最坏路径） | 0 | **31.8 ms** | ✅ |
| T3 模糊 `vsc`（T1+T2 稀疏 → 全表并行扫描） | 22 | **23.0 ms** | ✅ |
| 评分排序 `report`（T1 命中逐条评分 + 全量排序） | 78,607 | **110.3 ms** | ✅ |
| 对照：`fuzzy`/`pinyin` 关闭时的 `vsc` | 0 | **1.1 ms** | 开关生效，未付全表代价 |

### 三轮优化轨迹（同一脚本、同一数据）

| 轮次 | T2 拼音 | T3 模糊 | 评分排序 | 改动 |
|---|---:|---:|---:|---|
| 首测（工作树初态） | 813.1 ms | 927.1 ms | 978.0 ms | — |
| 第二轮 | 222.6 ms | 326.6 ms | 462.7 ms | 评分零分配、T2 rayon 并行、T3 `matches_folded`、`sub_ranges` 去拷贝 |
| 第三轮（终态） | **31.8 ms** | **23.0 ms** | **110.3 ms** | T2 加 `quick_may_match` 首字符必要条件剪枝 |

**说明**：T3 的计时包含前置 T2，前两轮的主要耗时其实来自 59 万条串行构造
拼音 token，而非模糊扫描本身；剪枝后 T3 真实全表成本才显现。剪枝是必要
条件（单测验证），不改变匹配语义。评分采用零分配 ASCII 不敏感比较，
与 DESIGN §15「不做完整 Unicode case folding」一致。

### 真实卷实机复核（release · `ff-cli` · C: 卷）

复现：`cargo build --release -p ff-cli` → `./target/release/ff-cli.exe search C: <查询> --max N --sort score`

| 查询 | 条目数 | 建索引 | 搜索 | 结果 |
|---|---:|---:|---:|---|
| `wd`（T1 子串） | 817,901 | 5.075 s | 4 ms | `wdmvsc.PNF` / `wdmaudio.PNF` / `wdc.dll` 等，路径与大小正确 |
| `report --sort score` | 817,901 | 3.961 s | 4 ms | 评分排序返回 `report` 目录，路径正确 |
| `zgr` | 817,904 | 3.917 s | 9 ms | 5 条均为**字面子串**命中（文件名含 `zgr`），非拼音命中；真实卷无「中国人」类名字 |
| `文档`（中文子串） | 817,904 | 3.920 s | 2 ms | `新建文本文档.txt` / `复习文档.md` 等 |
| **`wendang`（T2 拼音全拼）** | 817,903 | 3.934 s | **7 ms** | **命中 `文档模板.md` / `文档.mydocs` / `新建文本文档.txt` 等中文名** —— T2 在真实卷生效；末 2 条长中文书名是 T3 模糊命中（tier 分低于拼音，排在后面） |

**结论**：T1 / T2 / T3 与评分排序在真实 NTFS 卷上均跑通，7 ms 级响应。
`zgr` 无拼音命中说明精简字典覆盖有限（真实卷缺合适样本），不等于 T2 失效。

**未实测 / 边界**：多音字、双拼（精简字典未覆盖）、生产拼音库替换后的召回率；
GUI 端到端「按键→首屏 ≤50 ms」已于 2026-11-15 真机实测为 **145.6 / 146.2 / 146.8 ms**，
按端到端口径不达标（120 ms 防抖主导，引擎 6–7 ms），详见下方「GUI 真机验证」节与 DECISIONS #78/#80；
Top-K 部分选择未做（见 DECISIONS #71）。

### M5 门禁（2026-11-15）

`cargo fmt --all -- --check`（双 workspace）/ `cargo clippy --workspace --all-targets -D warnings` /
`cargo clippy --all-targets -D warnings`（`app/src-tauri`）/ `cargo check`（`app/src-tauri`）/
`cargo test --workspace` **208 passed** / `npm run build`（50 modules）/ release `m5probe` —— 全绿。
（此为 M5 提交当时的快照；同日收尾轮新增 3 个单测后为 **211 passed**，见下节。）

### M5 收尾轮：P0-2 / P0-3 / P1-1 验证（2026-11-15）

| 项 | 验证方式 | 真实结果 |
|---|---|---|
| P0-2 剪贴板 | `cargo test -p ff-engine --lib clipboard_roundtrip -- --nocapture`（Windows 真写 + 回读 + 恢复原内容） | `clipboard roundtrip OK: 写入并回读 53 字节`；测试含中文、emoji、反斜杠，1 passed |
| P0-2 失败透传 | 代码审阅 `clipboard_set_text`：`OpenClipboard` 5×10 ms 重试后仍失败 → `Err` → `onOpenError` | 非 Windows 分支返回 `Err("当前平台不支持系统剪贴板")`，不再静默 `Ok(())` |
| P0-3 菜单 / P1-1 拖拽 | `npm run build` 后 grep `app/dist/assets/*.js` | 命中 `data-tauri-drag-region`、`ctxmenu`、`copy_name`、`复制文件名`、`右键查看更多操作`；50 modules 构建绿 |
| 门禁 | fmt / clippy `-D warnings`（双 workspace）/ `cargo test --workspace` / `npm run build` | 全绿，`cargo test --workspace` **211 passed**（+3 个新单测） |

**同为 2026-11-15 的历史边界记录**：写这一节时浏览器原型验证受工具限制
（截图通道 >10 MiB、DOM 快照只回传结果行节点），因此当时未宣称 GUI 实测。
**当日稍后已由 Tauri 真机轮补齐，见下一节**；本段保留以说明为何分两步。

### GUI 真机验证（2026-11-15，DECISIONS #78–#80）

环境：`cargo build --release --manifest-path app/src-tauri/Cargo.toml
--features custom-protocol` → `sift-app.exe` 11 MB；启动带
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`；
WinSta0 交互桌面（桌面会话探测 `READY`）；真实卷 **2,179,900 条**索引。
Win32 `SendInput` 真实鼠标键盘 + CDP `Runtime.evaluate` / `Page.captureScreenshot`
双通道，脚本见 `.work/gui_act.py`、`.work/cdp.py`（本地，未入库）。

| 项 | 真实操作 | 判定结果 |
|---|---|---|
| P1-1 拖动 | 拖拽区（放大镜图标）按下并移动 (+200,+110) | 窗口 rect `[260,100,1033,587]` → `[452,206,1225,693]`（+192,+106）✅ |
| P1-1 负向对照 | 输入框内拖动 | rect 不变，输入框仍能真实键入 `report` ✅ |
| P1-1 ACL | 首测 `unhandledrejection` | `Command plugin:window\|start_dragging not allowed by ACL` → 补 capability 后错误数组为空 ✅ |
| P0-2 `Ctrl+C` | 输入框无选区（`selectionStart===selectionEnd===6`）时真实按键 | 系统剪贴板读回 `D:\Sift\.worktrees\d\target\criterion\report` + toast「已复制完整路径」✅ |
| P0-3 菜单 | 结果行真实右键 | 四项 + 快捷键，rect `[301,133,216,139]` 全在 760×480 视口内 ✅ |
| P0-3 `Esc` | 真实 Esc | `menu=false`、`input` 仍 `report`、`IsWindowVisible=true`（未清空、未隐藏）✅ |
| P0-3 键盘 | `End` + `Enter` | 高亮「复制文件名」→ 剪贴板 `report` + toast「已复制文件名」✅ |
| P0-3 鼠标 | 点第三项「复制完整路径」 | 剪贴板完整路径 + toast「已复制完整路径」✅ |
| 失败透传 | 结果集 60 s 过期后执行菜单项 | toast「请求不存在或已过期」，不是静默失败 ✅ |
| 首屏耗时 | 三轮「清空 → 真实键入 `report` → 首屏行渲染」 | **145.6 / 146.2 / 146.8 ms**（引擎 6–7 ms，120 ms 防抖主导）⚠️ |
| 引擎搜索 | 状态栏 `elapsed_ms` | 6–12 ms（真实卷 2,179,900 条，2,907–2,909 命中）✅ |

证据图（CDP 截图，可直接打开）：
- `docs/evidence/gui-report.png` — 搜索 `report` 首屏，含状态栏「右键查看更多操作」提示。
- `docs/evidence/gui-context-menu.png` — 四项右键菜单、快捷键、高亮与分隔线。
- `docs/evidence/gui-time-format-bug.png` — 真机发现的时间格式缺陷（见下）。

**真机新发现（DECISIONS #80；第 1、2 项已于同日修复，见下一节）**：
1. **（已修，#81）** `status.last_saved = 134392046912540464`（FILETIME，
   1601 纪元 100 ns），前端按 JS epoch 毫秒用 → `Invalid Date` → 修改时间列
   `NaN-NaN-NaN NaN:NaN`、状态栏「已保存 NaN:NaN」。
   `ResultRow.mtime` 注释要求 IPC 边界转换，实际未转。
2. **（已修，#81）** 目录行大小显示 `0 B`（UI_SPEC §4.2 要求空白）。
3. DESIGN §13-M3「≤50 ms」与 `SEARCH_DEBOUNCE_MS = 120` 字面冲突，
   端到端实测 146 ms；防抖后段约 26 ms 达标，口径待裁定。
4. 仍未实机覆盖：托盘菜单、全局热键物理触发、失焦隐藏稳定性、
   hide/show 循环、中文 IME 组词。

验证结束已恢复剪贴板原内容并关闭应用进程。

### P1-8 修复后真机复验（2026-11-15，DECISIONS #81）

同环境重建 release（新前端 `dist` + `app/src-tauri/src/timeconv.rs`），
真机卷 **2,182,600 条**；Win32 真实键盘 + CDP 直调 IPC / DOM 断言双通道。

| 项 | 验证方式 | 真实回显 |
|---|---|---|
| `status.last_saved` | CDP 直调 `invoke('status')` | `1794733215003`（13 位 epoch ms）→ `2026-11-15T09:00:15.003Z`（本地 17:00）；原 18 位 FILETIME 不复现 |
| `range` 时间字段 | 直调 `search`+`range`，`report` / `ext:txt` 各 20–30 行 | `invalidDates=0`、`nullMtime=0`；目录样例 `1792820494533` → `2026-10-24T05:41:34.533Z`；文件行同为 13 位 |
| 修改时间列 | 真实键入 `report` 后读 DOM | `10-12 18:46` / `2024-11-7 15:32` / `11-15 14:28`…，全文 `anyNaN=false` |
| 状态栏 | DOM `.status-right` | `索引就绪 · 2,182,600 条 · 已保存 17:00` |
| 目录大小 | `folder:report`（23 行全目录）与 `report` 混合结果 | 目录行 `.c-size` 全空；同屏文件行 `70 KB` / `11 KB` / `406 KB` / `10 KB`（未误清文件） |
| 首屏回归 | 三轮「清空 → 真实键入 → 首屏行」 | **149.6 / 144.9 / 143.2 ms**，与修复前 145.6 / 146.2 / 146.8 ms 同量级；每窗口 ≤30 次 i64 除法无可测开销 |

**本轮门禁**：根 workspace `cargo fmt --all -- --check` / `clippy --workspace
--all-targets -D warnings` / `cargo test --workspace` **211 passed**；
`app/src-tauri` fmt / clippy / **`cargo test` 7 passed**（`timeconv` 新增）；
`npm run test:format` **29 assertions**、`npm run build` 50 modules；
`cargo build --release --features custom-protocol` 成功出 11 MB `sift-app.exe`。

证据图：`docs/evidence/gui-time-fixed.png`（修复后，与 `gui-time-format-bug.png`
同为 `report` 场景）。**注意**：复验时交互桌面有外部输入/剪贴板串扰，
一次 `ctrl+a`+`type` 复合脚本把外部文本拼进了输入框；改为分步键入后
输入稳定为 `report`，并用 CDP 直调 IPC 独立复核，结论不受该噪声影响。

**仍开放**（不因本轮宣称完成）：#80 的首屏 50 ms 口径冲突、托盘菜单、
全局热键物理触发、失焦隐藏稳定性、hide/show 循环、中文 IME 组词。
