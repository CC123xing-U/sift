# Sift 项目评审说明书（给评审 AI 的完整材料）

> 生成时间：2026-10-26
> 代码基线：`git log` 最新提交 `1e2981a`（分支 `main`）
> 项目根目录：`D:\Sift`
>
> 本文是**现状说明书**，不是设计稿。设计意图请对照 `docs/DESIGN.md`（权威）、`docs/UI_SPEC.md`（界面规格）。
> 三者的关系是：**DESIGN（要什么） > UI_SPEC（长什么样） > 本文件（实际做成了什么）**。冲突时按此顺序取舍。
>
> **状态更新（2026-10-26 晚，提交 `415183b`）**：本文档写成后 **M4 查询语法与属性过滤已完成**，
> 其中 **P0-1（筛选 chips 全部报解析错误）已修复**——`ext:` / `folder:` 等修饰符引擎侧现已支持，
> 7 个 chips 全部可用。§3.2 的「查询解析」与 §5 的 P0-1 条目请按 M4 后的状态理解。
>
> **状态更新（2026-11-15，M5 完成）**：**M5 模糊 + 拼音 + 评分排序已接线并实测达标**
> （`InProcessEngine::search` 调 `search_ranked`，T1/T2/T3 分层 + `ScoreConfig` 评分，
> 200 万条实测 T2 31.8 ms / T3 23.0 ms / 评分 110.3 ms，见 `docs/BENCH.md` M5 节）。
> 设置页 `模糊匹配` / `拼音匹配` / `显示系统/隐藏文件` 三个开关已真正驱动引擎，
> 按 UI_SPEC §4.8.1 摘除「未接线」标注。§1.2、§1.4、§3.2、§3.6、§7 已按 M5 后状态更新；
> **§5 的 P0-2 复制路径、P0-3 右键菜单、P1-1 窗口拖动、P1-7 设置页其余项仍然有效**。
>
> **状态更新（2026-11-15，M5 后的缺陷收尾轮）**：**P0-2 复制路径、P0-3 右键菜单、
> P1-1 窗口拖动已修复**。引擎侧用 Win32 FFI 真实写系统剪贴板（含新增 `CopyName`），
> 结果行新增四项右键菜单与快捷键提示，`.topbar` 加 `data-tauri-drag-region="deep"`
> 让无边框窗口可拖动。§3.4 / §3.5 / §3.6 / §5 / §8 已按修复后状态更新；
> 决策与验证边界见 `docs/DECISIONS.md` #74–#77。**P1-7 设置页其余项仍然有效**。
>
> **状态更新（2026-11-15，GUI 真机验证轮）**：三项已在 Tauri release 真机
> （WinSta0 交互桌面 + WebView2 CDP）用真实鼠标键盘全部实测通过，
> 详见 `docs/BENCH.md`「GUI 真机验证」节与证据图 `docs/evidence/`。
> **重要更正**：真机首测发现 P1-1 只加 `data-tauri-drag-region` 并不足以拖动 ——
> Tauri ACL 报 `start_dragging not allowed by ACL`，补
> `app/src-tauri/capabilities/default.json` 的
> `core:window:allow-start-dragging` 后才真正生效（DECISIONS #79）。
> 真机另发现两项未修缺陷（FILETIME 未转导致时间列 `NaN`、目录大小显示 `0 B`）
> 与首屏 146 ms 的规范口径冲突，已记入 §5 P1-8 / DECISIONS #80。
> 当前工作树基线以 `git log` 为准，本文仍以「M3 原型期」视角写就的段落请按上述更新理解。

---

## 0. 请这样评审

### 0.1 建议阅读顺序

| 顺序 | 文件 | 为什么先读它 |
|---|---|---|
| 1 | 本文件 §1 | 先知道软件是干什么、给谁用、现在做到哪一步 |
| 2 | `docs/DESIGN.md` §1 目标与非目标、§13 里程碑 | 知道「本阶段**应该**有什么」；当前 M0–M5 已完成，避免用 M6/M7 的标准评判 |
| 3 | `docs/UI_SPEC.md` §2 设计 Tokens、§3 布局、§5 快捷键 | 界面评判的基准线 |
| 4 | 本文件 §3–§5 | 逐功能对照实现方法与已知缺陷 |
| 5 | `docs/DECISIONS.md` 第 63–73 条 | 最近几轮实机踩坑、M5 接线与性能实测记录，避免重复提已修问题 |
| 6 | `docs/AUDIT_M3_FINAL.md` | 上一轮另一位 AI 的代码审计报告（P0/P1 分级），避免重复 |

### 0.2 事实口径约定

- 本文所有实现描述均来自**当前工作树代码**，不是设计意图。凡未实机验证的，一律标 **【未验证】**。
- 凡代码里存在但**未接线 / 点了没效果**的，一律标 **【未接线】** 并说明点击后的真实后果。
- 不要凭文件名或注释判断功能可用——本文已逐条核对到实现代码。

### 0.3 希望你产出什么

1. **合理性判断**：每个功能的实现方式是否合理？有无更简洁/更正确的做法？
2. **交互设计判断**：窗口、布局、快捷键、状态反馈是否符合「搜索工具」的心智？缺失什么？
3. **缺口清单**：按 P0（用户会以为坏了）/ P1（明显不好用）/ P2（锦上添花）分级，给出**具体改法**（改哪个文件、哪个函数）。
4. **不要建议**引入大型依赖、重写架构、或提前做 M6/M7 的功能（服务拆分/HTTP API/对话框集成/内容搜索）——这些有明确排期。M5 的模糊/拼音/评分已完成，可正常评判其实现质量。

---

## 1. 这个软件是什么

### 1.1 一句话

**Sift 是一个 Windows 本地文件搜索工具**：按文件名（不是内容）在全盘建立内存索引，用全局热键唤出一个小窗口，边打字边出结果，回车直接打开。

### 1.2 对标与差异化

| 对标产品 | 学它什么 | 本文对应实现 |
|---|---|---|
| **Everything** | 内存索引的速度：读 NTFS 的 `$MFT` 建索引，用 USN Journal 做增量；百万级条目毫秒级出结果 | `ff-ntfs`（MFT 扫描 + USN 解析）+ `ff-core`（SoA 列存 + memchr/rayon 并行 memmem） |
| **Listary** | 匹配体验：模糊匹配、中文拼音（全拼/首字母）、评分排序 | **M5 已实现**：`ff-match`（T2 拼音 / T3 模糊）+ `ff-core::score`（ScoreConfig 评分排序）；当前用精简拼音字典，双拼 / 多音字未覆盖 |

### 1.3 典型使用场景（请据此评判交互）

用户正在任何程序里工作 → 按 `Ctrl+Alt+Space` → 屏幕中央弹出 760×480 的搜索窗口，输入框已聚焦 → 输入 `report` → 窗口内实时列出全盘匹配的文件 → `↑↓` 选 → `Enter` 打开；`Ctrl+Enter` 在资源管理器中选中该文件 → `Esc` 隐藏，回到原工作。

**设计取向**（`UI_SPEC.md` §1）：简约紧凑、工具优先、Windows 11 原生观感、信息密度高（单行 32px、一次 10+ 行）、无左侧导航栏、动效克制。

### 1.4 当前进度

| 里程碑 | 内容 | 状态 |
|---|---|---|
| M0 | 真实卷摸底 | ✅ 完成（数据见 §6） |
| M1 | 引擎核心（数据结构、并行子串搜索、多词 AND） | ✅ 完成 |
| M2 | 持久化 + USN 增量 | ✅ 完成（实机项部分待复核） |
| **M3** | **能用的 UI（Tauri 窗口 + 热键 + 托盘 + 虚拟列表）** | ✅ **完成，正在自用打磨** |
| M4 | 查询语法与属性过滤（`size:` `dm:` `ext:` `path:` `regex:`、OR/NOT/通配符） | ✅ 完成（106 条语法用例，C: 卷实测 2–69 ms） |
| M5 | 模糊 + 拼音 + 评分排序 | ✅ 完成（2026-11-15；T1/T2/T3 分层 + 评分排序，200 万条实测达标；规则编辑器 / 双拼 / 多音字未做） |
| M6 | 服务拆分（普通权限 UI + 提权服务）、命名管道、HTTP API | ❌ 未开始 |
| M7 | 对话框集成、内容搜索、硬链接、filelist 导入（**右键菜单已提前到 M5 收尾轮完成**） | ❌ 未开始 |

### 1.5 硬约束（评判时请考虑）

- **仅 Windows**，且当前**需要管理员权限**才能读 `$MFT` 与 USN Journal（M6 才拆分成普通权限 UI + 提权服务）。
- 单进程架构：Tauri 外壳内直接跑引擎（`InProcessEngine`），没有服务进程。
- 无网络、无遥测、无自动更新、无插件系统（明确非目标）。

---

## 2. 架构总览

### 2.1 分层与职责

| 层 | 位置 | 职责 | 平台约束 |
|---|---|---|---|
| `ff-core` | `crates/ff-core/` | 纯逻辑：SoA 索引数据结构（`VolumeIndex`）、查询解析、并行搜索、路径现算、匹配高亮、FFIX 持久化 | **零平台依赖，`#![forbid(unsafe_code)]`，任何平台可编译可单测** |
| `ff-match` | `crates/ff-match/` | 匹配器封装（`Matcher` trait）：T1 子串 / T2 拼音 / T3 模糊 | 同上，纯逻辑 |
| `ff-ntfs` | `crates/ff-ntfs/` | Windows 专有：卷枚举、`$MFT` 全量扫描、USN Journal 解析与增量、Win32 错误处理 | `#[cfg(windows)]` |
| `ff-engine` | `crates/ff-engine/` | 引擎编排：`api`（冻结的 IPC 契约）、`inprocess`（进程内引擎：搜索/取行/取消/状态/重建/打开）、`overlay`（Base+Overlay 增量合并） | 编排层，逻辑跨平台 |
| `ff-cli` | `crates/ff-cli/` | 命令行调试工具：`scan` / `search`，用于脱离 GUI 复现问题 | Windows 功能需管理员 |
| `sift-app` | `app/src-tauri/` | Tauri v2 外壳：窗口、全局热键、托盘、9 个 IPC 命令 | Windows |
| 前端 | `app/src/` | React 18 + TS + Vite：`EngineClient` 契约的两种实现（Tauri / Mock） | 浏览器可独立运行 |

### 2.2 进程与线程模型

```
┌─ sift-app.exe（单进程，管理员）──────────────────────────────┐
│                                                             │
│  Windows 消息循环 / Tauri 运行时                              │
│    ├─ WebView2 渲染 app/dist（React 前端）                    │
│    └─ invoke → 9 个 Rust 命令 → Arc<InProcessEngine>          │
│                                                             │
│  InProcessEngine（共享状态）                                  │
│    ├─ volumes: RwLock<HashMap<VolumeId, VolumeData>>          │
│    │     VolumeData = { info, index(SoA), overlay, state }    │
│    ├─ results: Mutex<HashMap<RequestId, StoredResult>>        │
│    └─ scanner: Mutex<Box<dyn VolumeScanner>>                  │
│                                                             │
│  线程：                                                       │
│    T1 启动扫描线程：逐卷 rescan（全量扫 MFT，数秒级）           │
│    T2 后台轮询线程：100ms 切片读 USN 增量，累积 ≥50,000 条      │
│                     触发 merge_overlay_into_base               │
│    T3 落盘节流：每 10s 检查一次，脏则原子写 .ffix              │
│    T4 结果回收：60s TTL 未访问的结果集                         │
└─────────────────────────────────────────────────────────────┘
```

**关键设计**：扫描与合并都在后台线程，搜索在主线程（invoke）同步执行，因此**索引未完成时也能搜索**（结果是部分的，见 §5 缺陷 P1-2）。

### 2.3 一次搜索的完整数据流

```
用户输入 "report"
   ↓ （前端 120ms 防抖）
TauriEngine.search({query, sort, opts})
   ↓ invoke('search') → JSON
InProcessEngine::search_impl
   ├─ ff_core::query::parse("report")  → Query{ terms: [Term{text:"report"}] }
   │    （语法不支持时返回 ParseError{ message, span }）
   ├─ 对每卷并行（rayon）：names_lower 大 blob 上 memchr::memmem 找子串
   ├─ 多词 AND：先搜最稀有的词，再对其结果集求交
   ├─ sort_hits（score/name/path/size/modified）
   └─ 存结果集，返回 SearchSummary{ id, total, elapsed_ms, parse_error }
   ↓
前端 setSummary → ResultArea 用 @tanstack/react-virtual 渲染
   ↓ 可视窗口变化（90ms 防抖）
invoke('range', {id, offset, limit}) → 只取当前屏幕的几十行（含高亮区间）
```

---

## 3. 功能清单：实现方式与现状

> 图例：✅ 可用 · ⚠️ 可用但有缺陷 · 🔌 未接线（代码存在但点了没效果）· ❌ 未实现

### 3.1 索引子系统

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 卷枚举 | ✅ | `ff-ntfs/src/win32.rs::enumerate_volumes` | `GetLogicalDrives` 位掩码逐盘符查序列号/文件系统 + `FindFirstVolumeW` 枚举无盘符的 GUID 装载点卷；返回 `VolumeInfo{serial, fs, guid_path, drive}`。后续 `has_mount_points` 过滤掉无挂载点的卷（上一轮审计修复） |
| 全量扫描建索引 | ✅ | `ff-ntfs/src/scanner.rs::NtfsVolumeScanner` | 打开卷设备 → `FSCTL_QUERY_USN_JOURNAL` 记录 `journal_id`/`next_usn` → 两遍读 `$MFT`（先收集 `RawEntry`，再解析父子关系）→ 流式写入 `VolumeIndex` SoA 列 |
| 索引数据结构 | ✅ | `ff-core`（`VolumeIndex`） | SoA 列存：`names` / `names_lower` 两个大 blob + offsets、`parents`、`sizes`、`mtimes`、`attrs`、`flags`（含 `HAS_CJK` 位，供 M5 拼音剪枝用）。路径**不存储**，`path_string()` 现算（含环检测） |
| USN 增量 | ✅ | `ff-ntfs/src/usn.rs` + `ff-engine/src/inprocess.rs::poll_all` | 后台线程 100ms 切片 `read_changes`（从 `next_usn` 续读）→ 解析 USN 记录 → `merge_usn_events` 合并成 create/delete/rename/move → 写入 Overlay |
| Overlay 合并 | ✅ | `ff-engine/src/overlay.rs` | Base（全量索引）+ Overlay（增量增删改）；累积变更 ≥ `MERGE_THRESHOLD = 50_000` 时 `merge_overlay_into_base`，合并期间不阻塞搜索 |
| Journal 失效重建 | ✅ | `inprocess.rs::rebuild_volume` | `read_changes` 返回 `JournalUnavailable` → 全量重建。上一轮修了 **Bug #6**：此前丢弃了 `full_scan` 返回的 `ScanMeta`（`journal_id`/`next_usn` 写成 0），导致轮询恒以 `from_usn=0` 续读 → 恒失效 → **无限循环重建**（已加回归测试 + 负向对照验证） |
| 索引持久化 | ✅ | `ff-core/src/persist.rs` + `inprocess.rs::save_all/load_all` | 自研 **FFIX** 格式：零依赖、自带 CRC32、`MAGIC=b"FFIX"`、版本 1、原子写（临时文件 + rename）、`load` 容错（校验失败返回 `None` → 触发重建）。存到 `%LOCALAPPDATA%\Sift\<serial>.ffix` |
| 落盘节流 | ⚠️ | `inprocess.rs::maybe_save`，`SAVE_INTERVAL = 10s` | 每 10s 检查一次并落盘。**缺陷：无脏标记**——稳态下即使无变更也会每 10s 重写一次索引（实机约 177MB/卷），见 §5 P1-5 |
| 启动载入 | ✅ | `main.rs::build_engine` | 先 `load_all` 载入已有索引（秒级，UI 立即可搜），再开后台线程逐卷全量重扫保证新鲜度 |
| 全局进度 | ⚠️ | 前端 `types.ts::overallScanProgress` | 按卷条目数加权。上一轮修了「进度回跳」：引擎新增 `InProcessEngine::mark_pending()`（inherent 方法，不动冻结 trait），扫描前把本轮要扫的卷全部置为 `Scanning`，否则未轮到的卷（索引已从磁盘载入、状态 Ready）被算作已完成 → 每开扫一卷全局进度被拽回低位。复算验证：修复前 `49→100→85.9→100→65→100`（2 次回跳），修复后 `0→100` 单调 |

### 3.2 搜索子系统

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 查询解析 | ✅ | `ff-core/src/query.rs::parse` | M4 AST（`And/Or/Not/Name/Pred`，NOT>AND>OR）+ 16 个修饰符；106 条表驱动语法用例，错误带字节 span 与中文原因 |
| 子串匹配 | ✅ | `ff-match/src/matcher.rs::SubstringMatcher` | `memchr::memmem` 封装（不自写 Boyer-Moore）；在 `names_lower` 大 blob 上并行扫描 |
| 多词 AND | ✅ | `ff-core/src/search.rs` | 稀有词优先：先搜命中数最少的 term，再对其结果集逐条验证其余 term |
| 匹配分层 | ✅ | `ff-core/src/score.rs::search_ranked` + `ff-match` | T1 子串（M4 AST）/ T2 拼音（只遍历 `cjk_ids`，首字符剪枝）/ T3 模糊（T1+T2<100 时并行全表，`matches_folded` 零分配）。Overlay 增量条目经 `match_overlay_name` 走同一套判定 |
| 评分排序 | ✅ | `inprocess.rs::sort_hits` | `SortKey::Score` 用 `ScoreConfig` 真实评分降序（同分 tie-break 来源+id），其余 4 键取真值；200 万条 / 78,607 命中实测 110.3 ms。**Top-K 部分选择未做**（见 DECISIONS #71） |
| 大小写 | ✅ | `ff-core/src/case.rs` | `case_sensitive=false` 时走 `names_lower` 列；`true` 时走原名。选项由 `App.tsx::searchOpts` 从设置页派生（不再是硬编码常量） |
| 请求取消 / 防错乱 | ✅ | `App.tsx::searchSeqRef` + `SearchSummary.id` | 每次 `runSearch` 自增序号，过期回包丢弃并主动 `cancel(id)` 释放结果集（否则要等 60s TTL 回收）；扫描完成的卷会触发当前查询重跑 |
| 取消令牌 | ✅ | `ff-core/src/cancel.rs` | `CancelToken`，搜索中可取消 |
| 结果集回收 | ✅ | `inprocess.rs::reap_stale`，`RESULT_TTL = 60s` | 60s 未被 `range`/`open` 访问的结果集自动释放 |

### 3.3 结果呈现

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 虚拟滚动 | ✅ | `app/src/components/ResultArea.tsx` | `@tanstack/react-virtual`，行高固定 32px，`total` 即虚拟列表总长度 |
| 分段取行 | ✅ | 同上 + `engine.range(id, offset, limit)` | 只拉可视窗口 + 缓冲区的行；**滚动 90ms 防抖**（`RANGE_DEBOUNCE_MS`），滚动中以已有数据渲染、缺失行渲染空白（避免满屏骨架闪烁） |
| 命中高亮 | ✅ | `ff-core/src/highlight.rs::highlight_name` + 前端 `NameHL` | Base / Overlay 共用 T1/T2/T3 区间判定；引擎把 UTF-8 字节偏移转成 **UTF-16 下标**（中文/emoji 不再错位）后返回 `name_matches: [[start,end],...]`，前端切片渲染 `<mark>`，底色 `#FFD500`。M4 的通配符 / 正则仍不高亮 |
| 列头排序 | ✅ | `ResultArea::onHeaderClick` | 点同一列切换升降序，点新列默认降序 |
| 状态面板 | ✅ | `ResultArea::EmptyState` | 5 种面板：`scanning`（索引中）/ `emptyQuery`（空查询）/ `searchingEmpty`（搜索中）/ `noResult`（无结果，带「清除筛选」按钮）/ `error`（索引错误，带「重试」→ `rescan`） |
| 骨架屏 | ✅ | `RowContent({skeleton})` | 仅「新搜索首屏且尚无任何行」时前 3 行显示骨架 |
| 顶部不确定进度条 | ✅ | `styles.css::.topprogress` | `transform: translateX` 动画（不用 `background-position`，符合 UI_SPEC §2.6「只动 opacity/transform」），周期 `--dur-progress: 1.4s` |

### 3.4 打开与操作

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 打开文件 | ✅ | `inprocess.rs::open_impl` → `shell_open(full, false)` | 用系统 Shell 关联程序打开完整路径 |
| 打开所在文件夹 | ✅ | 同上 → `shell_open(full, true)` | `explorer /select,"<full path>"`（在资源管理器中选中该文件） |
| 复制路径 | ✅ | `inprocess.rs::open_impl` → `clipboard_set_text(full)` | **2026-11-15 修复**：Win32 FFI 写 `CF_UNICODETEXT`（零新增依赖），失败返回 `Err` 并在状态栏提示；`Ctrl+C` 与右键菜单共用 `runAction`，成功后 toast「已复制完整路径」。真实剪贴板往返单测见 DECISIONS #74 |
| 复制文件名 | ✅ | 同上 → `clipboard_set_text(file_name_from_full(&full))` | 新增 `OpenAction::CopyName`，`types.ts` / `MockEngine.ts` 同步契约 |
| 双击行打开 | ✅ | `ResultArea` 行 `onDoubleClick` | |
| **右键菜单** | ✅ | `ResultArea.tsx` `.list::onContextMenu` + `.ctxmenu` | **2026-11-15 修复**：四项（打开 / 打开所在文件夹 / 复制完整路径 / 复制文件名），右侧显示快捷键；`role=menu` + ↑↓/Home/End/Enter/Esc 键盘可达；外部点击 / 滚动 / resize / 换结果集关闭；Esc 用 document capture 抢在全局 Esc 之前，不会顺带隐藏窗口 |
| 行内快捷按钮 | ❌ | — | 仍无 hover 出现的「打开文件夹」小图标；属 P2，本轮按 UI_SPEC §4.2 的四列 grid 结构不加 |

### 3.5 窗口与系统集成

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 主窗口 | ✅ | `app/src-tauri/tauri.conf.json` | 760×480，最小 480×320，居中，置顶（alwaysOnTop），可缩放，**`decorations: false`（无边框）**，标题「Sift」 |
| 全局热键 | ✅ | `main.rs` + `tauri-plugin-global-shortcut` | `CmdOrCtrl+Alt+Space` → `toggle_window`。底层是 `RegisterHotKey` 【未验证：合成按键无法触发，需物理按键测试】 |
| 双击 Ctrl 唤出 | ❌ | — | 需要 `WH_KEYBOARD_LL` 低级键盘钩子，未实现（设置页开关存在但无效） |
| 托盘 | ✅ | `main.rs::build_tray` | 菜单项：显示 / 隐藏 / 重建索引 / 分隔线 / 退出 【未验证：实际点击行为未实机测试】 |
| 失焦自动隐藏 | ⚠️ | `main.rs::on_window_event` | 两道守卫：① 窗口「曾获得过焦点」后才允许隐藏（修了启动即隐藏的 bug）；② 前台窗口属于**本进程**时不隐藏（WebView2 子窗口/输入法抢焦点会让父窗口收到 `WA_INACTIVE`，那不是用户切走）。**仍偏激进**，见 §5 P1-6 |
| Esc 隐藏 | ✅ | `App.tsx` | Esc 优先清空输入，输入已空 → `invoke('hide')`（Tauri）或隐藏 DOM（浏览器原型） |
| 窗口命令 | ✅ | `main.rs` | `show_window` / `hide_window` / `toggle_window` 三个 IPC 命令 |
| **窗口拖动** | ✅ 真机实测 | `TopBar.tsx` → `.topbar[data-tauri-drag-region="deep"]` **+ `app/src-tauri/capabilities/default.json` → `core:window:allow-start-dragging`** | **2026-11-15 修复（两个条件缺一不可）**：Tauri v2 `drag.js` 对 deep 沿 composedPath 上溯，遇到 `INPUT`/`BUTTON` 会自动阻止拖动，因此输入框选字、chips、✕/⚙ 仍可用，图标与空白区可拖；但仅有属性时 Tauri ACL 拒绝 `start_dragging`，补 capability 后才生效（DECISIONS #79）。真机拖拽后窗口 rect `[260,100,...]` → `[452,206,...]`，输入框内拖动为负向对照（窗口不动）。按 UI_SPEC §3.1 不加自绘关闭按钮 |

### 3.6 设置页（`app/src/components/Settings.tsx`）

> 设置页有 6 个分组：常规 / 索引 / 匹配 / 高级（占位）/ 语言 / 关于。
> **所有设置只存 React state，不落盘**（切换视图保留，重启丢失）。

| 设置项 | 状态 | 说明 |
|---|---|---|
| 开机自启 | 🔌 | 仅改内存状态，无行为 |
| 失去焦点自动隐藏 | 🔌 | 前端状态**未传给 Rust**（Rust 侧无条件启用） |
| 双击 Ctrl 唤出 | 🔌 | 未实现（需 `WH_KEYBOARD_LL`） |
| 全局热键（可录制） | 🔌 | 录制交互可用（拼出 `Ctrl+Alt+Space` 字符串），但**不会重新注册真实热键**（Rust 侧硬编码） |
| 索引卷（C:/D:/E: 勾选） | 🔌 | 硬编码 `ALL_VOLUMES = ['C:\\','D:\\','E:\\']`，**不是真实枚举结果**，勾选不影响实际扫描 |
| 排除路径 | 🔌 | 增删改交互完整（含 a11y 标签），**不传给引擎** |
| 显示系统/隐藏文件 | ✅ | **已接线**：`App.tsx::searchOpts` 按 `!开关` 成对派生 `hide_system` / `hide_hidden`（§11.3），标注已摘除 |
| 模糊匹配 / 拼音 | ✅ | **已接线**：直通 `SearchOptions.fuzzy` / `.pinyin`，驱动 T3 / T2 分层与高亮，标注已摘除 |
| 双拼方案 | 🔌 | 可见标注「暂未支持」；精简拼音字典不含双拼 |
| 评分规则编辑器 | ❌ | 显式禁用，可见标注「M5 预留，未实现」（§8.3 用户规则 / PriorFolders 未做） |
| HTTP API 开关与 token | ❌ | 显式禁用，标注「M6 提供」 |
| 界面语言（中文/English） | 🔌 | `i18n.ts` 目前只有中文资源表，切换无效果 |
| 版本 / 索引位置 / 许可 | ✅ | 静态展示 |

**标注现状（2026-11-15）**：三个已接线 toggle 按 `UI_SPEC.md` §4.8.1「接线后须移除标注」摘掉了 `pending-tag`；其余未接线项（开机自启 M6、失焦隐藏、双击 Ctrl、自定义热键、索引卷、排除路径、界面语言、双拼、规则编辑器）均按**各自真实阶段**补上可见小字标注，没有统一套用同一个标签。设置项仍只存 React state、不落盘（M6 服务化时补）。

### 3.7 IPC 契约（`crates/ff-engine/src/api.rs` ↔ `app/src/types.ts`）

9 个命令，`EngineError` 统一映射为 `String`：

| 命令 | 方向 | 载荷 |
|---|---|---|
| `search` | 前端→引擎 | `SearchRequest{query, sort, max_hits?, opts}` → `SearchSummary{id, total, elapsed_ms, parse_error}` |
| `range` | 前端→引擎 | `(id, offset, limit)` → `Vec<ResultRow>` |
| `cancel` | 前端→引擎 | `id`（fire-and-forget） |
| `status` | 前端→引擎 | → `EngineStatus{volumes[], total_entries, last_saved, searching}` |
| `rescan` | 前端→引擎 | `volume?: string`（**当前忽略该参数，重建全部**） |
| `open` | 前端→引擎 | `(id, index, action)`，`action ∈ {open, open_folder, copy_path}` |
| `show_window` / `hide_window` / `toggle_window` | 前端→外壳 | 无参数 |

**类型适配细节**（保留冻结契约的 Rust 类型不变，只在序列化层做手脚）：

| 字段 | Rust 类型 | 线上形态 | TS 类型 |
|---|---|---|---|
| `ParseError.span` | `Range<usize>` | `{start, end}` | `{start:number; end:number}` |
| `ResultRow.name_matches` | `Vec<Range<u32>>` | `[[start,end],...]` | `[number, number][]` |
| 枚举（`SortKey`/`OpenAction`/`VolumeState`） | Rust enum | `snake_case` 字符串（`#[serde(rename_all)]`） | 字面量联合 |
| `VolumeStatus.volume` | `String`（IPC 边界由 `VolumeId` 降维） | string | `string` |

前端**不依赖 `@tauri-apps/api`**——靠 `withGlobalTauri: true` 直连 `window.__TAURI__.core.invoke`，因此纯浏览器（`vite dev` + MockEngine）也能完整跑，方便 UI 迭代。

---

## 4. 前端界面设计

### 4.1 布局结构（`app/src/App.tsx`）

```
┌─ .app-window（760×480）───────────────────────────────┐
│ ┌─ TopBar ──────────────────────────────────────────┐ │
│ │ [🔍] 搜索文件与文件夹…            [✕] [⚙]         │ │  ← 输入行（autoFocus）
│ │ [全部][文件夹][文档][图片][视频][音频][压缩包]      │ │  ← chips（toolbar + roving tabindex）
│ │ ⚠ 解析错误提示条（有错误时才出现）                 │ │
│ └─┘ │
│ ┌─ ResultArea ──────────────────────────────────────┐ │
│ │ 名称 │ 路径 │ 大小 │ 修改时间        （列头可排序）│ │
│ │ ─────────────────────────────────────────────────  │ │
│ │ 📄 report_2024.pdf   D:\Work\Reports  1.2MB  …     │ │  ← 32px/行
│ │ 📁 Reports           D:\Work           —      …     │ │
│ │ …（虚拟滚动，滚动停止 90ms 后拉下一批行）           │ │
│ │                                                    │ │
│ │ 或：空状态 / 搜索中 / 无结果 / 索引中 / 错误面板    │ │
│ └─┘ │
│ ┌─ StatusBar ───────────────────────────────────────┐ │
│ │ 找到 1,234 个结果（3 ms）      索引就绪 · 180万 项 │ │
│ └─┘ │
└──────────────────────────────────────────────────────┘
```

**视图切换**：`view === 'settings'` 时整窗替换为设置页（不是弹窗/抽屉），Esc 或「返回」回到搜索视图。

### 4.2 组件清单

| 组件 | 文件 | 职责 |
|---|---|---|
| `TopBar` | `components/TopBar.tsx` | 输入行 + chips + 解析错误条。chips 用 `role="toolbar"` + roving tabindex（整组只占 1 个 Tab stop），`←→`/`Home`/`End` 在组内移动并阻止冒泡 |
| `ResultArea` | `components/ResultArea.tsx` | 列头 + 虚拟列表 + 5 种状态面板。通过 `forwardRef` 暴露 `ResultAreaHandle`（moveSelection/page/home/end/open/hasSelection）供 App 的键盘处理器调用 |
| `StatusBar` | `components/StatusBar.tsx` | 左侧搜索结果摘要，右侧索引状态。**渲染与无障碍播报分离**：可见节点 `aria-hidden`，另设 `sr-only` + `aria-live="polite"` 只念「找到 N 个结果」（不含耗时） |
| `Settings` | `components/Settings.tsx` | 6 个分组（见 §3.6） |
| `icons.tsx` / `i18n.ts` / `format.ts` / `queryUtils.ts` | `app/src/` | 内联 SVG 图标、中文文案资源表、大小/时间格式化、chips↔查询文本双向同步 |

### 4.3 设计 Tokens（`app/src/styles.css`）

- **双主题**：`color-scheme: light dark` + `prefers-color-scheme` 切换。深色用深灰分层（**不用纯黑**）。
- 色板：Win11 Fluent 中性灰阶（`--bg-app #F5F5F5` / `--bg-surface #FFFFFF` / `--bg-selected #E0EDF7` / `--accent #0067C0` / `--bg-highlight #FFD500` / `--error #B10E1C`）。
- 动效时长：`--dur-instant 80ms` / `--dur-fast 120ms` / `--dur-base 160ms` / `--dur-progress 1.4s`；只动 `opacity` / `transform`。
- **对比度已实测**（`UI_SPEC.md` §2.2，WCAG 2.1）。有一处主动修正记录：开关关闭态轨道原 `#BFBFBF` 在白底只有 1.84:1（低于非文字对比度 3:1），改为 `#8F8F8F`（3.23:1）。

### 4.4 快捷键（`App.tsx::onKeyDown`，全键盘可操作）

| 键 | 行为 |
|---|---|
| `↑` `↓` | 移动选中项（输入框聚焦时也生效） |
| `PageUp` / `PageDown` | 翻页（±15 行） |
| `Home` / `End` | 跳到首行 / 末行 |
| `Enter` | 打开选中项 |
| `Ctrl+Enter` | 打开所在文件夹（`explorer /select,`） |
| `Ctrl+C` | **复制完整路径（2026-11-15 已修复，见 §3.4）**，成功后状态栏 toast；输入框内有选区时走浏览器原生复制 |
| `Ctrl+F` | 聚焦并全选输入框 |
| `Esc` | 清空输入 → 再按隐藏窗口；设置页内则返回搜索视图 |
| 直接打字 | 焦点自动转入输入框（保留已输入内容） |
| `Ctrl+Alt+Space` | 全局唤出/隐藏（Rust 侧实现） |

### 4.5 无障碍

- 列表 `role="listbox"`，行 `role="option"` + `aria-selected` + `aria-posinset/setsize`，容器 `aria-activedescendant` 指向选中行。
- 列头按钮带完整 `aria-label`（含当前排序方向）；chips 用 `aria-pressed`。
- 设置页开关 `role="switch"` + `aria-checked`，占位项的 `aria-label` 含里程碑说明（满足 WCAG 2.5.3 Label in Name）。
- 渲染与播报分离（见 §4.2 StatusBar）。

---

## 5. 已知缺陷与未接线清单

> 分级：P0 = 用户会以为坏了 / 点了没反应；P1 = 明显不好用；P2 = 锦上添花。

### P0

> **2026-11-15 更新**：P0-1 已由 M4 修复（`ext:` / `folder:` 引擎侧已支持，7 个 chips 全部可用）；
> **P0-2 复制路径、P0-3 右键菜单、P1-1 窗口拖动已由 M5 收尾轮修复**
> （Win32 剪贴板 FFI + 四项右键菜单 + `data-tauri-drag-region`，见 DECISIONS #74–#76）。
> 下表保留评审原文以便追溯，问题描述为**修复前**状态，勿按现状理解。

| # | 问题 | 位置 | 具体后果 | 建议方向 |
|---|---|---|---|---|
| **P0-1** | **筛选 chips 全部触发解析错误** | `queryUtils.ts::applyChip` 生成 `ext:...` / `folder:` → `query.rs::validate_term` 遇 `:` 报错 | 点「文档/图片/视频/音频/压缩包/文件夹」任一 chip → 输入框填入含 `:` 的修饰符 → 引擎返回 `ParseError("修饰符语法（如 size:）尚未支持（计划 M4）")` → 顶部红条报错 + 结果区显示「没有找到…」。**7 个 chip 里 6 个不可用**（只有「全部」可用，它做的是清除修饰符） | 二选一：① M4 实现 `ext:`/`folder:` 修饰符（正解，chip 逻辑已就绪）；② 在 M4 前把 chips 隐藏或禁用并标注「M4 提供」 |
| **P0-2** ✅ 已修复 | **Ctrl+C 复制路径无效果** | `inprocess.rs::open_impl` → `OpenAction::CopyPath => Ok(())` | 按 `Ctrl+C` 什么都不会发生，剪贴板无变化 | 引擎侧用 `OpenAction::CopyPath` 写剪贴板（Windows `SetClipboardData`），或改由前端 `navigator.clipboard.writeText`（前端拿不到完整路径，需要 `range` 行数据拼 `dir + name`）。**采用左栏第一案**：Win32 FFI 零新增依赖，另补 `CopyName` 与成功 toast |
| **P0-3** ✅ 已修复 | **没有右键菜单** | `ResultArea.tsx` 无 `onContextMenu` | 「打开文件夹」「复制路径」只有键盘能触发，界面无任何提示，用户完全发现不了 | 加右键菜单（打开 / 打开所在文件夹 / 复制完整路径 / 复制文件名）+ hover 行内按钮 + 状态栏或输入框下方一行快捷键提示。**已实现菜单四项 + 菜单内快捷键 + 状态栏提示；hover 行内按钮属 P2，按 UI_SPEC §4.2 四列 grid 未加** |

### P1

| # | 问题 | 位置 | 说明 |
|---|---|---|---|
| P1-1 ✅ 已修复并真机实测 | **窗口无边框且不能拖动** | `tauri.conf.json` `decorations: false`，原无 `data-tauri-drag-region`、`capabilities/` 为空 | 窗口无法移动，也没有关闭/最小化按钮。用户只能用 Esc 隐藏或托盘退出。需在 TopBar 加拖拽区 + 自绘关闭按钮，或改回有边框。**已采用拖拽区方案**：`.topbar` 标 `data-tauri-drag-region="deep"`，**并补 capability 放行 `core:window:allow-start-dragging`**（真机首测的 ACL 拒绝是修复前的隐藏坑，DECISIONS #79）；按 UI_SPEC §3.1 不加快关闭按钮，其余标题栏按钮保持 ✕/⚙ |
| P1-2 | 索引未完成时搜索，UI 不标注「部分结果」 | `StatusBar` / `ResultArea` | 扫描中就能搜，但结果不完整且 UI 不说明，用户会误以为搜全了（正确性 + 体验双重问题） |
| P1-3 | 卷状态错误时只显示「索引出错：未知」 | `inprocess.rs::status_impl` 硬编码 `message: None` | `load_volume_with` 出错只设 `state = Error`，不存 message。需要把错误串透传到 `VolumeStatus.message` |
| P1-4 | 首轮扫描（无历史索引）进度分母为 0 | `overallScanProgress` 用 `entries` 加权 | 条目数未知时 `w = max(1, entries) = 1`，进度会长期停在 0% 再跳 100%。需要粗粒度估算（如按已扫卷数 / 总卷数，或按已处理 MFT 字节数） |
| P1-5 | 落盘无脏标记 | `maybe_save`，`SAVE_INTERVAL = 10s` | 稳态下每 10s 重写一次索引（实机约 177MB/卷），白白吃 IO。加 dirty 标记即可 |
| P1-6 | 失焦隐藏仍偏激进 | `main.rs::on_window_event` | 已有两道守卫，但用户实测启动后几秒就被别的窗口「抢焦点」而隐藏。建议加「点击外部不隐藏」开关，或延长宽限期 |
| P1-7 | 设置页选项全部不生效、不落盘 | `Settings.tsx` + `main.rs` | 见 §3.6。用户点了开关，界面有反馈但行为不变 |
| **P1-8（真机新发现，未修）** | 时间列 / 状态栏出现 `NaN-NaN-NaN NaN:NaN`、「已保存 NaN:NaN」；目录行大小显示 `0 B` | `inprocess.rs:950` 把 Windows FILETIME 原样送 IPC；`types.ts::ResultRow.mtime` 注释要求转 JS epoch 毫秒却没转；`ResultArea.tsx::RowContent` 未判 `is_dir` | CDP 实测 `status.last_saved = 134392046912540464`（18 位，1601 纪元 100 ns），`new Date()` → Invalid Date。证据图 `docs/evidence/gui-time-format-bug.png`，根因与修复方向见 DECISIONS #80。目录 `0 B` 违反 UI_SPEC §4.2「目录显示空白」 |

### P2

| # | 问题 | 说明 |
|---|---|---|
| P2-1 | 幽灵卷 | 实机枚举出 2 个无挂载点的卷（1–2ms 就「完成」）。已用 `has_mount_points` 过滤，但 UI 仍可能显示 0 条目的卷 |
| P2-2 | `rescan` 忽略盘符参数 | 引擎无按盘符查卷的 API，托盘「重建索引」与设置页按卷勾选都只能全量重建 |
| P2-3 | 窗口位置/大小不记忆 | 每次启动都是配置的默认值 |
| P2-4 | 长路径只做尾部省略 | `ResultArea` 用 `title` + CSS 省略。更长路径宜做中间省略（`C:\…\中间\末尾`） |
| P2-5 | 空查询时无示例 | 可给可点的示例查询（M4 语法就绪后价值更大） |
| P2-6 | 落盘目录在 C 盘满时静默失败 | 索引写 `%LOCALAPPDATA%\Sift\`，C 盘无空间时失败但 UI 不提示（本机 C 盘曾 0 字节可用） |
| P2-7 | 搜索历史 / 常用查询 / 多选批量操作 | 均无 |

---

## 6. 实测性能数据（同一台机器，非估算）

**基准机器**：Windows 10 专业工作站版 x64 / AMD Ryzen 5 5600U（6 核 12 线程）/ 15.3 GB / HP SSD EX900 1TB；工作区在 D: 卷；采集日 2026-10-24。

| 场景 | 条目数 | 实测值 |
|---|---:|---:|
| 真实卷全量建索引（C:，含 USN 查询 + MFT 两遍） | 849,366 | **5.312 s**（峰值 Working Set 142.66 MiB） |
| 真实卷端到端 `rescan` 建 SoA 索引 | 878,732 | **4.803 s** |
| 真实卷子串搜索 `ntdll` | 878,732 | **1 ms** |
| 真实卷多词 AND `system dll` | 878,729 | **3 ms** |
| 合成建索引 | 1,000,000 | 619.03 ms（Criterion 中央估计） |
| 合成子串 `report` | 1,000,000 | 1.3159 ms |
| 合成 AND `report summary` | 1,000,000 | 1.5136 ms |
| 合成 200 万条目内存 | 2,000,000 | 238.5 MB（峰值 255.5 MB） |
| 朴素基线（M0，逐条 `to_lowercase().contains`） | 849,366 | 88.022 ms |

**尚未实测**（注明，别当事实）：按键→首屏 ≤50ms 的 GUI 端到端计时、重启载入 ≤0.5s、文件变更 ≤1s 可搜到、合并期间搜索 p99、强制 kill 后索引可载入。这些都需管理员 + 真实 NTFS 卷 + WebView2 实机，不在 CI。

---

## 7. 路线图（请按此判断「哪些建议属于超前」）

| 里程碑 | 内容 |
|---|---|
| **M4** | 查询语法与属性过滤：第 7 节解析器 + `size:` `dm:` `ext:` `path:` `regex:`、OR/NOT/通配符/分组、按列排序、Top-K。**验收：语法测试集 ≥80 条** |
| **M5** | 模糊匹配 + 拼音（全拼/首字母）+ 评分排序 + 高亮范围 — **2026-11-15 核心完成并实测达标**；双拼、多音字、设置页规则编辑器（§8.3 用户规则）未做 |
| **M6** | 服务拆分（普通权限 UI + 提权服务）、命名管道、本地 HTTP API（含安全要求）、自动重连 |
| **M7** | 对话框集成（`SetWinEventHook` + UI Automation）、内容全文搜索、硬链接多路径、filelist 导入、非 NTFS 回退遍历、资源管理器右键菜单 |

**明确不做**（DESIGN §1.2）：ETP/FTP 服务端、gRPC、遥测、自动更新框架、插件系统、云同步、WebView 内嵌网页、多语言框架（只中文 + 英文）、非 Windows 平台。

---

## 8. 请重点回答这些问题

1. **P0-1（chips 全废）的最佳处置**：在 M4 之前，是隐藏 chips、禁用 chips，还是保留并标注「M4 提供」？哪种对用户伤害最小？
2. **P0-3（右键菜单）** —— **已定（2026-11-15）**：菜单项 = 打开 / 打开所在文件夹 / 复制完整路径 / 复制文件名，右侧显示快捷键；提前到 M5 收尾轮实现，不待 M7。理由与实现见 DECISIONS #75。
3. **P1-1（无边框窗口不能拖动/无关闭按钮）** —— **已定（2026-11-15）**：保留无边框，用 `.topbar` 拖拽区解决移动；不加快自绘标题栏与关闭按钮（UI_SPEC §3.1 明确「无最小化/最大化/关闭」），关闭语义仍由 Esc / 失焦 / 托盘承担。理由见 DECISIONS #76。
4. **交互心智**：当前「设置页替换整个窗口」的做法是否合理？还是应该用浮层/抽屉？
5. **状态反馈**：扫描中的「部分结果」应该怎么呈现才不误导用户？StatusBar 的信息密度合适吗？
6. **信息架构**：四列（名称/路径/大小/修改时间）在 760px 宽度下的分配是否合理？是否该加「类型」列或详情面板（`UI_SPEC.md` §3.5 提过右侧详情面板，当前未实现）？
7. **快捷键**：`Ctrl+C` 复制路径、`Ctrl+Enter` 打开文件夹是否符合 Windows 用户习惯？有无更标准的映射？
8. **缺失功能**：站在「一个每天用 Everything/Listary 的重度用户」角度，当前 M3 阶段还缺什么会让你用不下去？请按 P0/P1/P2 分级，并说明每个建议属于哪个里程碑。

---

## 附：本机环境约束（复现问题时注意）

- 工作区在 `D:\Sift`，**C 盘曾出现 0 字节可用**，导致 `%TEMP%` 写入失败 → cargo 构建报「拒绝访问」。构建时建议 `export TMP=/f/Temp TMPDIR=/f/Temp TEMP=/f/Temp`。
- 引擎日志写到 `%TEMP%\sift-debug.log`（当前是临时调试代码，位于 `main.rs::dbg_log`，定位完成后会移除）。
- 运行 GUI 必须用带 `custom-protocol` 的 release 构建：`cargo build --release --features custom-protocol`。
  **原因**：Tauri v2 的 `cfg(dev)` 不是按 debug/release 判定，而是看 `tauri` 依赖有没有开 `custom-protocol` feature —— 不带该 feature 的构建会去请求 `http://localhost:5173`（devUrl）导致白屏。详见 `docs/DECISIONS.md` 第 63 条。
- 脱离 GUI 复现引擎问题最快的方式：**`ff-cli`**（`cargo run -p ff-cli -- scan C: ntdll` / `-- search`），可以把 Tauri/WebView2 因素整个隔离掉。
