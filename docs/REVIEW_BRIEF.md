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
> 7 个 chips 全部可用。§3.2 的「查询解析」与 §5 的 P0-1 条目请按 M4 后的状态理解；
> 其余缺陷条目（P0-2 复制路径、P0-3 右键菜单、P1 各项）仍然有效。

---

## 0. 请这样评审

### 0.1 建议阅读顺序

| 顺序 | 文件 | 为什么先读它 |
|---|---|---|
| 1 | 本文件 §1 | 先知道软件是干什么、给谁用、现在做到哪一步 |
| 2 | `docs/DESIGN.md` §1 目标与非目标、§13 里程碑 | 知道「本阶段**应该**有什么」，避免用 M5/M7 的标准评判 M3 |
| 3 | `docs/UI_SPEC.md` §2 设计 Tokens、§3 布局、§5 快捷键 | 界面评判的基准线 |
| 4 | 本文件 §3–§5 | 逐功能对照实现方法与已知缺陷 |
| 5 | `docs/DECISIONS.md` 第 63–66 条 | 最近几轮实机踩坑与修复记录，避免重复提已修问题 |
| 6 | `docs/AUDIT_M3_FINAL.md` | 上一轮另一位 AI 的代码审计报告（P0/P1 分级），避免重复 |

### 0.2 事实口径约定

- 本文所有实现描述均来自**当前工作树代码**，不是设计意图。凡未实机验证的，一律标 **【未验证】**。
- 凡代码里存在但**未接线 / 点了没效果**的，一律标 **【未接线】** 并说明点击后的真实后果。
- 不要凭文件名或注释判断功能可用——本文已逐条核对到实现代码。

### 0.3 希望你产出什么

1. **合理性判断**：每个功能的实现方式是否合理？有无更简洁/更正确的做法？
2. **交互设计判断**：窗口、布局、快捷键、状态反馈是否符合「搜索工具」的心智？缺失什么？
3. **缺口清单**：按 P0（用户会以为坏了）/ P1（明显不好用）/ P2（锦上添花）分级，给出**具体改法**（改哪个文件、哪个函数）。
4. **不要建议**引入大型依赖、重写架构、或提前做 M5/M7 的功能（拼音/模糊/评分/HTTP API/内容搜索）——这些有明确排期。

---

## 1. 这个软件是什么

### 1.1 一句话

**Sift 是一个 Windows 本地文件搜索工具**：按文件名（不是内容）在全盘建立内存索引，用全局热键唤出一个小窗口，边打字边出结果，回车直接打开。

### 1.2 对标与差异化

| 对标产品 | 学它什么 | 本文对应实现 |
|---|---|---|
| **Everything** | 内存索引的速度：读 NTFS 的 `$MFT` 建索引，用 USN Journal 做增量；百万级条目毫秒级出结果 | `ff-ntfs`（MFT 扫描 + USN 解析）+ `ff-core`（SoA 列存 + memchr/rayon 并行 memmem） |
| **Listary** | 匹配体验：模糊匹配、中文拼音（全拼/首字母）、评分排序 | **尚未实现**，排期 M5。当前只有子串 + 多词 AND |

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
| M4 | 查询语法与属性过滤（`size:` `dm:` `ext:` `path:` `regex:`、OR/NOT/通配符） | ❌ 未开始 |
| M5 | 模糊 + 拼音 + 评分排序 | ❌ 未开始 |
| M6 | 服务拆分（普通权限 UI + 提权服务）、命名管道、HTTP API | ❌ 未开始 |
| M7 | 对话框集成、内容搜索、硬链接、filelist 导入、右键菜单 | ❌ 未开始 |

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
| 查询解析 | ⚠️ | `ff-core/src/query.rs::parse` | **仅 M1 子集**：空白分隔 = AND、`"..."` 短语、名字子串。遇到 `|` `!` `()` `<>` `*?` `:` `\` `/` **干净返回 `ParseError`**（带字节 span 与「计划 M4」的中文提示） |
| 子串匹配 | ✅ | `ff-match/src/matcher.rs::SubstringMatcher` | `memchr::memmem` 封装（不自写 Boyer-Moore）；在 `names_lower` 大 blob 上并行扫描 |
| 多词 AND | ✅ | `ff-core/src/search.rs` | 稀有词优先：先搜命中数最少的 term，再对其结果集逐条验证其余 term |
| 匹配分层 | ❌ | `ff-match/src/matcher.rs::Tier` | trait 与分层已定义（T1 子串 / T2 拼音 / T3 模糊），**T2、T3 未实现**，排期 M5 |
| 评分排序 | ⚠️ | `inprocess.rs::sort_hits` | 5 个排序键（score / name / path / size / modified）均已接线且列头可点。**但 `Score` 当前没有真实评分**，按「卷来源 + EntryId 升序」近似（M5 才做评分） |
| 大小写 | ✅ | `ff-core/src/case.rs` | `case_sensitive=false` 时走 `names_lower` 列；`true` 时走原名。**注意：前端硬编码 `DEFAULT_OPTS` 里的 6 个选项，设置页无法修改** |
| 请求取消 / 防错乱 | ✅ | `SearchSummary.id` 单调递增 | 前端只接受 `id > lastAcceptedId` 的回包，旧回包丢弃并主动 `cancel(id)` 释放结果集（否则要等 60s TTL 回收） |
| 取消令牌 | ✅ | `ff-core/src/cancel.rs` | `CancelToken`，搜索中可取消 |
| 结果集回收 | ✅ | `inprocess.rs::reap_stale`，`RESULT_TTL = 60s` | 60s 未被 `range`/`open` 访问的结果集自动释放 |

### 3.3 结果呈现

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 虚拟滚动 | ✅ | `app/src/components/ResultArea.tsx` | `@tanstack/react-virtual`，行高固定 32px，`total` 即虚拟列表总长度 |
| 分段取行 | ✅ | 同上 + `engine.range(id, offset, limit)` | 只拉可视窗口 + 缓冲区的行；**滚动 90ms 防抖**（`RANGE_DEBOUNCE_MS`），滚动中以已有数据渲染、缺失行渲染空白（避免满屏骨架闪烁） |
| 命中高亮 | ✅ | `ff-core/src/highlight.rs` + 前端 `NameHL` | 引擎返回 `name_matches: [[start,end],...]`（UTF-8 字符区间），前端切片渲染 `<mark>`，高亮底色 `#FFD500` |
| 列头排序 | ✅ | `ResultArea::onHeaderClick` | 点同一列切换升降序，点新列默认降序 |
| 状态面板 | ✅ | `ResultArea::EmptyState` | 5 种面板：`scanning`（索引中）/ `emptyQuery`（空查询）/ `searchingEmpty`（搜索中）/ `noResult`（无结果，带「清除筛选」按钮）/ `error`（索引错误，带「重试」→ `rescan`） |
| 骨架屏 | ✅ | `RowContent({skeleton})` | 仅「新搜索首屏且尚无任何行」时前 3 行显示骨架 |
| 顶部不确定进度条 | ✅ | `styles.css::.topprogress` | `transform: translateX` 动画（不用 `background-position`，符合 UI_SPEC §2.6「只动 opacity/transform」），周期 `--dur-progress: 1.4s` |

### 3.4 打开与操作

| 功能 | 状态 | 实现位置 | 实现方法 |
|---|---|---|---|
| 打开文件 | ✅ | `inprocess.rs::open_impl` → `shell_open(full, false)` | 用系统 Shell 关联程序打开完整路径 |
| 打开所在文件夹 | ✅ | 同上 → `shell_open(full, true)` | `explorer /select,"<full path>"`（在资源管理器中选中该文件） |
| 复制路径 | 🔌 | `inprocess.rs::open_impl` | **`OpenAction::CopyPath => Ok(())`——什么都没做**。注释写着「由 UI 层处理」，但 `TauriEngine.open` 只是 `invoke('open')`，前端也没有剪贴板回退。**`Ctrl+C` 当前无任何实际效果** |
| 双击行打开 | ✅ | `ResultArea` 行 `onDoubleClick` | |
| **右键菜单** | ❌ | — | **完全没有实现**。没有 `onContextMenu`，没有菜单组件。这是用户明确反馈的缺口 |
| 行内快捷按钮 | ❌ | — | 没有 hover 出现的「打开文件夹」小图标等 |

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
| **窗口拖动/关闭按钮** | ❌ | — | `decorations: false` 且代码里**没有 `data-tauri-drag-region`**，也没有自绘的标题栏/关闭按钮。**窗口不能拖动，只能靠 Esc 隐藏或托盘退出** |

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
| 显示系统/隐藏文件 | 🔌 | UI 标注「M3 接线后生效」，实际未接。前端硬编码 `DEFAULT_OPTS.hide_system = hide_hidden = true` |
| 模糊匹配 / 拼音 | 🔌 | UI 标注「M5 生效」，M5 才实现 |
| 双拼方案 | 🔌 | M5 |
| 评分规则编辑器 | ❌ | 显式禁用，标注「M5 提供」 |
| HTTP API 开关与 token | ❌ | 显式禁用，标注「M6 提供」 |
| 界面语言（中文/English） | 🔌 | `i18n.ts` 目前只有中文资源表，切换无效果 |
| 版本 / 索引位置 / 许可 | ✅ | 静态展示 |

**值得肯定的做法**：未接线项在**可见 UI 上**直接标注生效里程碑（不是只写在文档或 title 里），且每个占位项标注不同的里程碑（不统一套用同一个标签）——符合 `UI_SPEC.md` §4.8.1 的硬性要求。

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
| `Ctrl+C` | **复制路径（当前无效，见 §3.4）**；输入框内有选区时走浏览器原生复制 |
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

| # | 问题 | 位置 | 具体后果 | 建议方向 |
|---|---|---|---|---|
| **P0-1** | **筛选 chips 全部触发解析错误** | `queryUtils.ts::applyChip` 生成 `ext:...` / `folder:` → `query.rs::validate_term` 遇 `:` 报错 | 点「文档/图片/视频/音频/压缩包/文件夹」任一 chip → 输入框填入含 `:` 的修饰符 → 引擎返回 `ParseError("修饰符语法（如 size:）尚未支持（计划 M4）")` → 顶部红条报错 + 结果区显示「没有找到…」。**7 个 chip 里 6 个不可用**（只有「全部」可用，它做的是清除修饰符） | 二选一：① M4 实现 `ext:`/`folder:` 修饰符（正解，chip 逻辑已就绪）；② 在 M4 前把 chips 隐藏或禁用并标注「M4 提供」 |
| **P0-2** | **Ctrl+C 复制路径无效果** | `inprocess.rs::open_impl` → `OpenAction::CopyPath => Ok(())` | 按 `Ctrl+C` 什么都不会发生，剪贴板无变化 | 引擎侧用 `OpenAction::CopyPath` 写剪贴板（Windows `SetClipboardData`），或改由前端 `navigator.clipboard.writeText`（前端拿不到完整路径，需要 `range` 行数据拼 `dir + name`） |
| **P0-3** | **没有右键菜单** | `ResultArea.tsx` 无 `onContextMenu` | 「打开文件夹」「复制路径」只有键盘能触发，界面无任何提示，用户完全发现不了 | 加右键菜单（打开 / 打开所在文件夹 / 复制完整路径 / 复制文件名）+ hover 行内按钮 + 状态栏或输入框下方一行快捷键提示 |

### P1

| # | 问题 | 位置 | 说明 |
|---|---|---|---|
| P1-1 | **窗口无边框且不能拖动** | `tauri.conf.json` `decorations: false`，无 `data-tauri-drag-region` | 窗口无法移动，也没有关闭/最小化按钮。用户只能用 Esc 隐藏或托盘退出。需在 TopBar 加拖拽区 + 自绘关闭按钮，或改回有边框 |
| P1-2 | 索引未完成时搜索，UI 不标注「部分结果」 | `StatusBar` / `ResultArea` | 扫描中就能搜，但结果不完整且 UI 不说明，用户会误以为搜全了（正确性 + 体验双重问题） |
| P1-3 | 卷状态错误时只显示「索引出错：未知」 | `inprocess.rs::status_impl` 硬编码 `message: None` | `load_volume_with` 出错只设 `state = Error`，不存 message。需要把错误串透传到 `VolumeStatus.message` |
| P1-4 | 首轮扫描（无历史索引）进度分母为 0 | `overallScanProgress` 用 `entries` 加权 | 条目数未知时 `w = max(1, entries) = 1`，进度会长期停在 0% 再跳 100%。需要粗粒度估算（如按已扫卷数 / 总卷数，或按已处理 MFT 字节数） |
| P1-5 | 落盘无脏标记 | `maybe_save`，`SAVE_INTERVAL = 10s` | 稳态下每 10s 重写一次索引（实机约 177MB/卷），白白吃 IO。加 dirty 标记即可 |
| P1-6 | 失焦隐藏仍偏激进 | `main.rs::on_window_event` | 已有两道守卫，但用户实测启动后几秒就被别的窗口「抢焦点」而隐藏。建议加「点击外部不隐藏」开关，或延长宽限期 |
| P1-7 | 设置页选项全部不生效、不落盘 | `Settings.tsx` + `main.rs` | 见 §3.6。用户点了开关，界面有反馈但行为不变 |

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
| **M5** | 模糊匹配 + 拼音（全拼/首字母/双拼）+ 评分排序 + 设置页规则编辑器 + 高亮范围 |
| **M6** | 服务拆分（普通权限 UI + 提权服务）、命名管道、本地 HTTP API（含安全要求）、自动重连 |
| **M7** | 对话框集成（`SetWinEventHook` + UI Automation）、内容全文搜索、硬链接多路径、filelist 导入、非 NTFS 回退遍历、资源管理器右键菜单 |

**明确不做**（DESIGN §1.2）：ETP/FTP 服务端、gRPC、遥测、自动更新框架、插件系统、云同步、WebView 内嵌网页、多语言框架（只中文 + 英文）、非 Windows 平台。

---

## 8. 请重点回答这些问题

1. **P0-1（chips 全废）的最佳处置**：在 M4 之前，是隐藏 chips、禁用 chips，还是保留并标注「M4 提供」？哪种对用户伤害最小？
2. **P0-3（右键菜单）**：菜单项应该有哪些？是否需要在 M4 语法就绪前就加上？
3. **P1-1（无边框窗口不能拖动/无关闭按钮）**：是加自绘标题栏（拖拽区 + 关闭按钮），还是改回系统边框？考虑到这是一个「唤出即用、用完即走」的搜索窗口。
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
