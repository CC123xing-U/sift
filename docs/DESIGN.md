# Sift 设计与开发指令（交给开发 AI 的唯一权威文档）

> 项目暂名 **Sift**（可改）。一个 Windows 本地文件搜索工具：
> **Everything 式的内存索引与速度 + Listary 式的匹配体验（模糊 / 拼音 / 评分排序）**。
>
> **本文档是权威。** `docs/` 下的三份分析文档（`01-Everything-功能实现文档.md`、`02-Listary-功能实现文档.md`、`03-重写指南-取长补短.md`）只作背景参考；
> 两者冲突时，以本文档为准。第 2.3 节列出了参考文档里**不要照抄**的内容。

---

## 0. 给 AI 的工作规则（先读这一节）

1. **一次只做一个里程碑**（第 13 节）。完成并通过验收后停下，汇报结果，等我确认再进入下一个。不要提前实现后面的功能。
2. **不确定的 API 不要猜。** 引入任何 crate 前先读它在 docs.rs 的文档或源码，确认版本与接口；并把名称、版本、许可证写进 `docs/DEPENDENCIES.md`。
3. **不得擅自引入第 2 节依赖清单之外的重量级依赖**（数据库、网络框架、异步运行时之外的大型库）。需要时先提出理由，等我同意。
4. **先写测试再写实现**（至少 `ff-core` 的纯逻辑部分）。每次提交前必须通过 `cargo fmt`、`cargo clippy -- -D warnings`、`cargo test`。
5. **平台隔离**：所有 Windows 专有代码放在 `ff-ntfs`、`ff-service` 和 `app/src-tauri` 中，并用 `#[cfg(windows)]` 包住。`ff-core`、`ff-match` 必须能在任何平台编译和单测。
6. **`unsafe` 必须有注释**说明不变量；Win32 调用统一封装成带 RAII 的安全包装（句柄自动关闭）。
7. **性能数字必须实测。** 每个里程碑结束时把测量结果追加到 `docs/BENCH.md`（机器配置、条目数、耗时）。没测过的不要写成事实。
8. **遇到本文档没覆盖的决策**，选最简单的方案并在 `docs/DECISIONS.md` 记一行（决定、理由）；影响架构的才来问我。
9. 修改本文档里写死的决策（第 2 节）之前，必须先问我。

---

## 1. 目标与非目标

### 1.1 目标

- 在 NTFS 卷上建立**全盘文件名索引**（名字、父子关系、大小、时间、属性），用 USN Journal 做增量更新。
- 输入时**逐字实时搜索**：子串、多词 AND、布尔语法、属性过滤、模糊匹配、中文拼音（全拼 / 首字母）。
- 结果按**评分排序**（匹配质量 + 上下文 + 用户规则），并支持按列排序。
- 一个搜索窗口（全局热键 / 双击 Ctrl 唤出）+ 托盘；后期提供命名管道与本地 HTTP API。
- 写索引不阻塞搜索；搜索可被下一次按键取消。

### 1.2 非目标（明确不做）

- 内容全文搜索（延后到 M7，之前不要碰）。
- ETP / FTP 服务端、gRPC、遥测（Sentry / Rudder）、自动更新框架、插件系统、云同步、WebView 内嵌网页。
- 多语言框架（只做中文 + 英文字符串，直接用一个简单的资源表）。
- 非 Windows 平台。

### 1.3 规模目标（须实测，可在评审后调整）

| 指标 | 目标 |
|---|---|
| 条目规模 | 200 万条 |
| 全量建索引（SSD，管理员） | 不劣于旧版 FlashFind 基线 69 秒，目标 ≤ 30 秒 |
| 启动载入已有索引 | ≤ 0.5 秒（旧版 0.26 秒） |
| 子串搜索（无 `path:`） | ≤ 100 毫秒（旧版 26–182 毫秒） |
| 内存占用（200 万条） | ≤ 300 MB（待实测） |
| 文件变更到可搜到 | ≤ 1 秒 |

---

## 2. 关键决策

### 2.1 技术栈（已定）

| 层 | 选型 | 备注 |
|---|---|---|
| 语言 | **Rust**（stable，edition 2021 或更新） | 引擎、服务、Tauri 后端 |
| 桌面外壳 | **Tauri v2** | 托盘、全局热键、窗口管理用官方插件 |
| 前端 | **React + TypeScript + Vite** | 虚拟滚动用 `@tanstack/react-virtual` |
| NTFS 读取 | **`ntfs-reader`** crate（MFT 全量 + USN） | 备选：`windows` crate 直接调 `FSCTL_*` |
| Win32 | **`windows`** crate（官方绑定） | 钩子、服务、命名管道、图标 |
| 子串搜索 | `memchr`（`memmem`）+ `rayon` | 不要自写 Boyer-Moore |
| 模糊匹配 | **`nucleo-matcher`** | 已被 Helix 编辑器长期使用 |
| 拼音匹配 | **`ib-pinyin`**（匹配）+ **`pinyin`**（转换，如需） | 支持简拼、全拼、多音字、双拼 |
| 序列化 | `rkyv`（优先，可 mmap）或 `bincode`，外加 `zstd` | 先用简单方案跑通，再优化 |
| 快照切换 | `arc-swap` | 读无锁 |
| 哈希表 | `rustc-hash`（`FxHashMap`） | |
| HTTP | `axum`（仅 M6，仅监听 127.0.0.1） | |
| 日志 | `tracing` + `tracing-subscriber` | |
| 服务 | `windows-service` | 仅 M6 |
| 测试 / 基准 | 标准 `cargo test` + `criterion` | |

**为什么不是 C# / WPF + Rust FFI（Listary 的做法）：** 两套工具链，跨语言边界调试成本高。
**为什么不是 RocksDB / LSM：** 目标是 Everything 式纯内存索引，不需要数据库；见 2.3。

### 2.2 核心架构决策

1. **引擎是独立 crate，通过 trait 对外**。UI 只依赖 `EngineClient` trait，有两种实现：`InProcess`（M1–M5，直接调引擎）和 `Pipe`（M6，经命名管道连服务）。这样后期拆服务不需要重写 UI。
2. **索引 = 不可变基础快照（Base）+ 小型增量层（Overlay）**。搜索读 `Arc<Base>` 和当前 Overlay；USN 事件只写 Overlay；后台线程在 Overlay 过大或空闲时合并出新 Base，用 `arc-swap` 原子切换。这样每个文件事件不需要复制 200 万条数据，搜索也永远不被写阻塞。
3. **路径不存储，按父链现算。** 每个条目只存 `parent` 索引，因此目录重命名 / 移动时子树路径自动正确，不需要批量改写。
4. **匹配用小写副本，显示用原名。** 名字存两份（原名 arena、小写 arena）。Windows 文件名大小写不敏感，默认搜索不区分大小写。
5. **UI 与引擎之间用"句柄 + 窗口取数"。** `search()` 返回 `total` 和请求句柄，UI 虚拟滚动时按 `offset/limit` 调 `range()` 取可见行，**不要一次把几十万条结果序列化给前端**。
6. **每次按键取消上一次搜索**（协作式取消令牌，`AtomicBool`，搜索循环里定期检查）。
7. **默认以管理员权限开发运行**（M0–M5 在管理员终端里跑，或给 exe 加 `requireAdministrator` 清单）。**服务拆分放 M6**，但接口从第一天起按 2.2-1 设计。

### 2.3 参考文档里**不要照抄**的内容

| 参考文档里的说法 | 本项目的做法 | 原因 |
|---|---|---|
| Listary 用 RocksDB / LSM 存索引 | 纯内存 + 整块序列化落盘 | 不需要数据库；启动更快；依赖更少 |
| gRPC / tonic / prost | 进程内调用；M6 用命名管道 + JSON | 单机场景太重 |
| 共享内存 + 注册窗口消息（UI↔HookHost） | 不做 | 没有注入式 HookHost |
| Everything 的"493 条一块""前向差分名字" | 名字 arena + 结构数组 | 这是逆向出的内部细节，不是需求 |
| WM_COPYDATA 文本 IPC | 命名管道（JSON）+ 本地 HTTP；WM_COPYDATA 仅作可选兼容 | 现代方案更易测试 |
| 对话框钩子 = DLL 注入 | 若做则用 `SetWinEventHook` + UI Automation（不注入） | 避免 32/64 位、杀软误报、维护成本 |
| 手写 MFT 解析 | 用 `ntfs-reader` | 属性列表、fixup、碎片化记录是 bug 重灾区 |
| cnmatch.bin 等二进制数据 / 图标 / 资源 | 不使用 | 只参考功能描述，不复用其数据文件；拼音数据来自开源库 |
| CRDT 变更集配置（Preferences.json） | 普通 JSON 配置 | 不做多设备同步 |

---

## 3. 仓库布局

```
sift/
├─ Cargo.toml                    # workspace
├─ crates/
│  ├─ ff-core/     索引数据结构 · 查询解析 · 求值 · 评分 · 持久化格式（纯逻辑，无 Windows 依赖）
│  ├─ ff-match/    子串 / 模糊 / 拼音 匹配器封装（统一 trait，纯逻辑）
│  ├─ ff-ntfs/     NTFS 全量扫描 + USN 增量（封装 ntfs-reader / windows），输出 RawEntry / Change
│  ├─ ff-engine/   把以上粘合：卷管理、Base+Overlay、后台合并、落盘调度、对外 trait 实现
│  ├─ ff-ipc/      协议类型（serde）· 命名管道 server/client · HTTP server（M6）
│  ├─ ff-service/  Windows 服务外壳（M6）
│  └─ ff-cli/      命令行调试工具：scan / search（已实现）；bench / dump 待后续里程碑（见 DECISIONS #40）
├─ app/            Tauri v2（src-tauri/ 与 React TS 前端 src/）
├─ bench/          合成数据生成器 + criterion 基准
└─ docs/
   ├─ DESIGN.md    （本文件）
   ├─ DECISIONS.md  BENCH.md  DEPENDENCIES.md
   ├─ 01-Everything-功能实现文档.md / 02-Listary-功能实现文档.md / 03-重写指南-取长补短.md（三份原始分析文档，只读）
   └─ ui-reference/（界面参考图）
```

依赖方向：`ff-engine → ff-ntfs, ff-core, ff-match`；`ff-ipc → ff-engine`；`app → ff-engine 或 ff-ipc`。`ff-core` 与 `ff-match` 不依赖任何其他本项目 crate（`ff-match` 可被 `ff-core` 依赖或并列，保持无环）。

---

## 4. 数据模型（ff-core）

### 4.1 条目表（结构数组，每卷一份）

```rust
pub type EntryId = u32;                 // 卷内索引；NONE = u32::MAX
pub struct VolumeIndex {
    pub volume: VolumeInfo,             // 盘符/GUID路径、序列号、journal_id、next_usn
    // —— 名字 ——
    pub names: Vec<u8>,                 // UTF-8，各名字首尾相连，条目间以 0x00 分隔
    pub names_lower: Vec<u8>,           // 同布局的小写副本，用于匹配；偏移与 names 完全一致
    // —— 条目列（SoA，长度均为 entry_count）——
    pub name_off: Vec<u32>,             // 名字在 names 中的起点（长度 = 到下一个 0x00）
    pub parent: Vec<u32>,               // 父条目；根为 NONE
    pub flags: Vec<u16>,                // IS_DIR | HIDDEN | SYSTEM | READONLY | HAS_CJK | STALE_META ...
    pub frn: Vec<u64>,                  // 文件引用号（NTFS：取低 48 位作为键，屏蔽序列号）
    pub size: Vec<u64>,
    pub mtime: Vec<i64>,                // FILETIME 或 unix 微秒，选一个并全局统一
    pub ctime: Vec<i64>,
    pub atime: Vec<i64>,
}
```

要求：
- `names_lower` 通过 Unicode 简单小写（`char::to_lowercase` 取单字符结果，多字符展开的取原字符）生成；**长度与 `names` 必须逐字节相同**，否则偏移失效——若某字符小写后字节数变化，则保留原字符。需要单元测试覆盖（含 `İ`、`ß`、中日韩字符）。
- 哈希表 `FxHashMap<u64, EntryId>`（FRN → 条目）**不落盘**，载入后重建（重建很快，也避免格式膨胀）。
- `HAS_CJK` 标志在建索引时设置；同时维护 `cjk_ids: Vec<EntryId>`（含汉字的条目列表），供拼音匹配只遍历这部分。
- 目标内存：200 万条 ≤ 300 MB（实测后记录到 BENCH.md）。

### 4.2 路径生成

```rust
fn write_path(&self, id: EntryId, buf: &mut Vec<u8>)   // 沿 parent 链向上，反向拼接；根前缀 "C:\"
```
- 复用线程本地缓冲区，避免每个条目分配。
- 深度上限（如 512）防止损坏数据造成死循环；检测到环要报告并跳过该条目。

### 4.3 增量层（Overlay）

```rust
pub struct Overlay {
    pub added_or_changed: FxHashMap<u64 /*frn*/, OverlayEntry>, // 新建或改名/移动后的条目（完整字段）
    pub deleted: FxHashSet<u64>,                                // 在 Base 中被删除的 frn
}
```
- 搜索时：遍历 Base（跳过 `deleted` 和被 `added_or_changed` 覆盖的 frn），再遍历 Overlay 条目。
- Overlay 条目的 `parent` 以 **FRN** 表示（父可能在 Base 也可能在 Overlay），路径生成时先查 Overlay 再查 Base。
- **合并（compaction）触发**：Overlay 条目数 > 50,000，或距上次合并 > 10 分钟且 Overlay 非空且系统空闲。合并在后台线程构建新 Base，完成后 `arc-swap` 切换，并清空已合并的 Overlay 部分（合并期间新到的事件保留）。
- 大小 / 时间：USN 记录**不包含**文件大小，增量条目的 `size/mtime` 初始标记 `STALE_META`，由后台线程批量用 `GetFileInformationByHandleEx` / `std::fs::metadata` 补齐；补齐前显示为空，不影响搜索。

---

## 5. 索引构建（ff-ntfs）

### 5.1 输出契约

```rust
pub struct RawEntry { pub frn: u64, pub parent_frn: u64, pub name: String,
    pub is_dir: bool, pub attrs: u32, pub size: u64, pub mtime: i64, pub ctime: i64, pub atime: i64 }

pub enum Change {
    Upsert(RawEntry),            // 新建、重命名、移动、属性变化（同 frn 覆盖）
    Remove { frn: u64 },
}

pub trait VolumeScanner {
    fn full_scan(&self, vol: &VolumeId, sink: &mut dyn FnMut(RawEntry)) -> Result<ScanMeta>; // 返回 journal_id / next_usn
    fn read_changes(&self, vol: &VolumeId, from_usn: i64, journal_id: u64) -> Result<(Vec<Change>, i64 /*new next_usn*/)>;
}
```
`ff-engine` 只依赖这个 trait；`ff-core` 的测试用一个内存假实现。

### 5.2 全量扫描

- 枚举固定 NTFS 卷（`GetLogicalDrives` + `GetVolumeInformationW` 判断文件系统；`FindFirstVolume` 取 GUID 路径）。
- 用 `ntfs-reader` 读 `\\.\X:` 的 MFT。**实现前先读该 crate 的 README / docs.rs 确认当前版本接口**（编写本文时是 0.4.x，其文档中有用 `Vec` 缓存路径比不缓存快约 3 倍的数据，但我们不依赖它的路径缓存，因为用父链现算）。
- 扫描开始前**先记录** `FSCTL_QUERY_USN_JOURNAL` 的 `next_usn` 与 journal id，再扫 MFT；扫描期间的变化通过随后从该 `next_usn` 续读来补。**顺序不能反。**
- 扫描过程流式喂给索引构建器，边读边写列；完成后排序/整理（不要求排序，但需保证 `parent` 解析完成；找不到父的条目挂到根下并记录警告计数）。
- 跳过：`$MFT`、`$Bitmap` 等元数据文件可保留（Everything 也能搜到），但默认可在设置中隐藏。排除规则（如 `$Recycle.Bin`）在建索引时应用。

### 5.3 增量更新（USN）

- 用 `FSCTL_READ_USN_JOURNAL` 从保存的 `next_usn` 续读，并带上 journal id。
- **关键坑（必须处理并测试）：**
  - 记录版本 V2（64 位 FRN）与 V3（128 位 FRN，ReFS）。本项目**只支持 NTFS 的 V2**；遇到 V3 记录时记录日志并忽略，不崩溃。
  - 读到文件末尾时返回 `ERROR_HANDLE_EOF` 是**正常结束**，不是错误。
  - FRN 含序列号（高 16 位）：用作键时统一 `& 0x0000_FFFF_FFFF_FFFF`。
  - 同一文件的多条 USN 记录要按 reason 合并：`FILE_CREATE`、`FILE_DELETE`、`RENAME_OLD_NAME`、`RENAME_NEW_NAME`、`CLOSE` 等；重命名以 `RENAME_NEW_NAME` 记录为准，旧名记录忽略。
  - 日志不存在或被禁用：尝试 `FSCTL_CREATE_USN_JOURNAL`；失败则该卷退化为"仅启动时全量重扫"并告知用户。
- **重建条件**（任一满足就丢弃该卷索引并全量重建，不要尝试修补）：
  - 保存的 journal id 与当前不一致；
  - 保存的 `next_usn` 小于当前日志的 `FirstUsn`（日志已被截断）；
  - 索引文件的格式版本不匹配或校验失败；
  - 卷序列号变化。
- 单独的监听线程每卷一个，批量读取（缓冲区 64 KB 起），事件批量提交到 Overlay（每批一次加锁 / 一次 swap Overlay，而不是每条一次）。

### 5.4 非 NTFS / 网络盘（延后）

M0–M6 只支持本地 NTFS。FAT/exFAT/ReFS/网络路径的目录遍历回退（`walkdir` 或 `ignore`）放 M7，并注意：不跟随符号链接 / 目录联接（junction）以避免环。

---

## 6. 持久化（ff-core）

- 文件位置：开发期 `%LOCALAPPDATA%\Sift\`；服务模式 `%ProgramData%\Sift\`。每卷一个 `index-<volume-serial>.bin`。
- 布局：`magic("FFIX")` · `format_version(u32)` · 卷元数据（序列号、journal id、next_usn、条目数）· 各列块（每块 `zstd` 压缩）· 末尾整文件校验（`xxh3` 或 `crc32`）。
- **原子写**：写 `*.tmp` → `flush/sync_all` → `std::fs::rename` 覆盖正式文件（Windows 上 std 的 rename 会替换已存在的目标）。任何时刻崩溃都不会留下半个正式文件。
- 载入：校验 magic / 版本 / 校验和，失败即视为无索引（触发重建），**不得 panic**。
- 保存时机：全量扫描完成后；合并（compaction）后；Overlay 脏且距上次保存 > 10 分钟；正常退出 / 服务停止时。保存在后台线程，不阻塞搜索。
- 格式版本号从 1 开始，每次改格式递增；不做向后兼容迁移，不兼容就重建。
- 优化路径（M2 之后再考虑，不是首版要求）：用 `rkyv` + mmap 实现零拷贝载入。

---

## 7. 查询语言（ff-core）

### 7.1 语法（自有规范，参考 Everything 用户习惯）

- 空格 = AND；`|` = OR；`!` = NOT；`"..."` = 含空格的短语；`(...)` 或 `<...>` 分组。
- 优先级：`NOT` > `AND` > `OR`。
- 默认：不区分大小写；对**文件名**做子串匹配；若 term 含路径分隔符 `\` 或 `/`，则对**完整路径**匹配。
- 通配符：`*`、`?`（仅在 term 含有它们时启用，否则走子串快速路径）。

### 7.2 修饰符与函数

| 阶段 | 内容 |
|---|---|
| M4 必做 | `case:` `nocase:` `file:` `folder:` `ext:a;b` `path:` `regex:` `wholeword:`（`ww:`）`startwith:` `endwith:` `size:` `dm:` `dc:` `da:` `parent:` `infolder:` `depth:` `attrib:` |
| M7 再说 | `content:` 族、`dupe:`、`runcount:`、`len:` 等 |

- `size:`：比较符 `> >= < <= =`，范围 `a..b`，单位 `b kb mb gb tb pb`（1024 进制），例：`size:>10mb`、`size:1kb..1mb`。
- 日期：`dm:today`、`dm:yesterday`、`dm:2024-01-01`、`dm:2024-01-01..2024-06-30`、`dm:last7days`；统一按本地时区解释。
- `regex:` 使用 `regex` crate（先用 `regex` 默认，不引入 `fancy-regex`），按条目名（或带 `path:` 时按路径）匹配。

### 7.3 解析与求值

- 手写递归下降解析器（或 `winnow`），产出 AST。**错误要有位置信息**，UI 可以显示"语法错误"但不崩溃；不完整的输入（用户正在打字）要尽量容错地降级为字面子串。
- 求值策略（成本递增顺序）：
  1. 取 AST 中所有"可名字级匹配"的 term，用 **一次大 `memmem` 扫描**加速：对 `names_lower` 整块并行扫描（按 0x00 边界切块给 `rayon`），命中偏移经二分 `name_off` 映射回 `EntryId`。
  2. 多个 AND 词：用最"稀有"（最长）的词先做候选集，其余词只在候选上校验。
  3. 属性过滤（size/date/flags）在候选集上用列扫描评估，向量化友好。
  4. 路径类 term（`path:`、含 `\`）最后在候选上现算路径比较；若整条查询只有路径 term，则全表并行扫描并使用线程本地路径缓冲。
- 结果表示：`Vec<EntryId>`（跨卷时使用 `(volume_idx, EntryId)` 打包为 `u64`）。
- 取消：所有长循环每处理约 4K 条检查一次取消令牌。

---

## 8. 匹配与评分（ff-match + ff-core）

### 8.1 分层匹配（决定哪些条目命中，并给出 tier）

| Tier | 条件 | 说明 |
|---|---|---|
| T1 | 子串 / 多词 AND（含前缀、全词、完全相等的子类） | 永远执行，最快 |
| T2 | 拼音（`HAS_CJK` 条目；查询为纯 ASCII 小写字母时触发） | 使用 `ib-pinyin`；默认开启简拼 + 全拼；双拼方案设为设置项，默认关闭 |
| T3 | 模糊（`nucleo-matcher`，字符序匹配） | 仅当 ① 设置开启（默认开启）且 ② T1+T2 命中数 < 100 且 ③ 查询长度 ≥ 2 时执行；先用 nucleo 内置的快速预过滤；结果带位置 |

- 大写字母的查询在拼音 tier 中**只按字面匹配**（`ib-pinyin` 的约定），用户用大写强制排除拼音。
- 拼音只遍历 `cjk_ids`，不遍历全表。
- 模糊匹配的全表耗时**必须实测**（BENCH.md）。若 200 万条下超过 150 毫秒，则：并行化 + 限制只对"名字长度 ≤ 64"的条目 + 超时返回部分结果。

### 8.2 评分（`ScoreConfig` 结构体，所有权重可调，集中在一个文件）

```
score = tier_base
      + position_bonus     // 名字内命中位置越靠前越高；完全相等 > 前缀 > 词首 > 其他
      + contiguity_bonus   // 连续命中 > 间断（模糊 tier 用 nucleo 自身分数缩放）
      + wholeword_bonus + case_exact_bonus
      + name_vs_path       // 命中在文件名 > 仅命中在路径
      - length_penalty     // 名字越短越靠前（同分时更相关）
      - depth_penalty      // 路径越浅越靠前
      + prior_folder_bonus // 在"最近使用目录"内加分（见 8.3）
      + rule_bonus         // 用户规则（扩展名 / 文件夹 / 文件名正则 / 属性）
      + frecency_bonus     // 用户打开过的次数 × 近期衰减（M5 先留接口，空实现）
```

- 排序只取 **Top-K 个**（K = 默认 10,000 个用于"按评分排序"的结果窗口）：使用 `select_nth_unstable_by` 或固定大小堆，**不要对数百万命中做全量排序**。
- **按列排序**（名字 / 路径 / 大小 / 修改时间）：对全部命中用 `rayon::par_sort_unstable_by_key`；需要比较路径时先批量生成排序键，不要在比较函数里现算路径。
- 评分单元测试（金标准，可随调参更新，但必须有）：
  - 完全相等 > 前缀 > 词首子串 > 中间子串 > 模糊；
  - 命中文件名 > 仅命中路径；
  - 其他条件相同，路径更浅的在前；
  - `vsc` 能模糊命中 `Visual Studio Code`；
  - `pysousuoeve` 命中 `拼音搜索Everything`；`zgr` 命中 `中国人`。

### 8.3 上下文与用户规则（M5）

- **PriorFolders**：维护"最近打开 / 最近搜索选中的目录"列表（来自用户在本程序中打开的结果；M7 可补"当前资源管理器目录"）。命中条目位于这些目录下则加分。
- **规则**（设置页配置，热更新，不需重启）：`FolderPriority`（路径前缀 → 加/减分）、`FileNamePriority`（正则 → 加/减分）、`ExtensionPriority`、`AttributePriority`。存为普通 JSON。

---

## 9. 引擎对外接口（ff-engine）

```rust
pub struct SearchRequest {
    pub query: String,
    pub sort: SortSpec,              // Score | Name | Path | Size | Modified，升/降
    pub max_hits: Option<usize>,     // 评分排序时的 Top-K
    pub opts: SearchOptions,         // case, regex_default, fuzzy, pinyin, hide_system ...
}

pub struct SearchSummary { pub id: RequestId, pub total: usize, pub elapsed_ms: u32, pub parse_error: Option<ParseError> }

pub struct ResultRow {
    pub name: String, pub dir: String,              // dir = 完整父路径
    pub is_dir: bool, pub size: Option<u64>, pub mtime: Option<i64>,
    pub name_matches: Vec<Range<u32>>,              // 供前端高亮，仅为取数窗口内的行计算
}

pub trait EngineClient: Send + Sync {
    fn search(&self, req: SearchRequest) -> Result<SearchSummary>;       // 自动取消同一客户端的上一次搜索
    fn range(&self, id: RequestId, offset: usize, limit: usize) -> Result<Vec<ResultRow>>;
    fn cancel(&self, id: RequestId);
    fn status(&self) -> EngineStatus;                                   // 各卷状态：Scanning(进度) / Ready / Error，条目数，最后保存时间
    fn rescan(&self, volume: Option<VolumeId>) -> Result<()>;
    fn open(&self, id: RequestId, index: usize, action: OpenAction) -> Result<()>; // 打开 / 所在文件夹 / 复制路径
}
```
- 会话中保存的命中集合（`Vec<u64>`）在新搜索到来、或 `cancel`、或超时（60 秒无访问）时释放。
- `search` 必须尽快返回 `total`；高亮、图标等昂贵信息只在 `range` 中为窗口内行生成。

---

## 10. IPC 与 HTTP API（M6）

### 10.1 命名管道（UI ↔ 服务）

- 管道名：`\\.\pipe\sift-<当前用户 SID>`；创建时设置 ACL，只允许该用户与 SYSTEM 访问。
- 协议：4 字节小端长度前缀 + JSON（`serde_json`）；消息类型对应 `EngineClient` 的方法；每个请求带 `request_id`，响应带相同 id。
- 服务重启或管道断开时，UI 自动重连并重新订阅状态。

### 10.2 本地 HTTP API（默认关闭，设置里开启）

- `GET /search?q=<query>&offset=0&count=100&sort=score` → JSON：`{ "total": N, "offset": 0, "results": [ { "name", "path", "is_dir", "size", "mtime" } ] }`。
- **安全要求（硬性）**：
  1. 仅绑定 `127.0.0.1`；
  2. 启用时生成随机 token，要求 `Authorization: Bearer <token>`（不要放在 URL 查询串里，避免进日志）；
  3. 校验 `Host` 头必须是 `127.0.0.1:<port>` 或 `localhost:<port>`（防 DNS rebinding）；
  4. 不设置宽松 CORS；
  5. 限制 `count` 上限（如 1000）与请求速率。
- 为什么这么严：文件列表本身是敏感信息，任何网页脚本都能请求 localhost。

---

## 11. UI 规格（app/）

### 11.1 窗口与交互

- 主窗口：无边框、居中、深浅色跟随系统；顶部输入框 + 下方结果列表 + 底栏状态（命中数、耗时、索引状态）。
- 列：名称（含图标）、路径、大小、修改日期；可调宽、可点击表头排序；列宽 / 窗口尺寸写入设置。
- 结果列表**必须虚拟滚动**；一次渲染只请求可见窗口 ±1 屏的数据。
- 快捷键：`↑/↓` 选择；`Enter` 打开；`Ctrl+Enter` 打开所在文件夹并选中；`Ctrl+C` 复制完整路径；`Alt+Enter` 属性（M7）；`Esc` 清空 / 隐藏窗口；`Ctrl+F` 聚焦输入框；`Tab` 在输入框与列表间切换。
- 匹配高亮：只对可见行，使用 `ResultRow.name_matches`。
- 图标：按扩展名缓存（`SHGetFileInfo`），异步加载，先显示占位。
- 输入防抖：**不防抖**，每次按键都发搜索并取消上一次；UI 只渲染最新请求 id 的结果。

### 11.2 唤出方式

- 全局热键（可配置，默认**不要用 `Alt+Space`**，它与系统窗口菜单冲突；默认可用 `Ctrl+Alt+Space`，由设置修改）。
- **双击 Ctrl**：低级键盘钩子（`WH_KEYBOARD_LL`），仅当两次 Ctrl 的"按下→释放"在 300ms 内且中间无其他按键才触发；钩子回调必须极快，只做时间戳判断，其余通过通道交给主线程。可在设置中关闭。
- 托盘图标：显示 / 隐藏、重建索引、设置、退出；开机自启（设置项）。
- 失去焦点自动隐藏（设置项，默认开）。

### 11.3 设置页（最小集）

索引卷选择 · 排除路径 · 隐藏系统/隐藏文件 · 热键 / 双击 Ctrl 开关 · 模糊 / 拼音开关与双拼方案 · 评分规则（M5）· HTTP API 开关与 token（M6）· 语言（中 / 英）。

**「隐藏系统/隐藏文件」是一个 toggle，同时驱动两个引擎字段**：`SearchOptions.hide_system`（过滤 `flags::SYSTEM`，`FILE_ATTRIBUTE_SYSTEM` 0x4）与 `SearchOptions.hide_hidden`（过滤 `flags::HIDDEN`，`FILE_ATTRIBUTE_HIDDEN` 0x2），两者默认均为 `true`。引擎侧保持两个独立判断而非合并成一个掩码，便于后续若把 UI 拆成两个 toggle 时不必再动引擎。

---

## 12. 权限与服务形态

- **M0–M5**：应用以管理员运行（开发时用管理员终端；打包时 `requireAdministrator` 清单）。不要在此阶段做服务。
- **M6**：拆分
  - `ff-service`（LocalSystem，开机自启）：持有引擎、索引、监听 USN、落盘、命名管道服务端、HTTP 服务端；
  - `app`（普通用户权限）：只作为 `EngineClient::Pipe` 的客户端；
  - 服务状态对 UI 可见（未安装 / 已停止 / 运行）；安装 / 卸载由安装器或一个需要提权的 `ff-cli service install|uninstall` 完成。
- 打开文件、"打开所在文件夹"这类动作由 **UI 进程**以当前用户身份执行（不要让服务以 SYSTEM 身份替用户打开文件）。

---

## 13. 里程碑与验收（逐个推进）

> 每个里程碑：实现 → 测试 → 在 `docs/BENCH.md` 记录实测 → **停下汇报**。

### M0 摸底（0.5–1 天）
- 在 `ff-cli` 里用 `ntfs-reader` 枚举一个 NTFS 卷，打印条目数、全量耗时、峰值内存；做一个朴素子串搜索，记录耗时。
- **验收**：BENCH.md 写入真实数据；确认 `ntfs-reader` 能给出 size / 时间 / 父子关系；列出它的限制（例如硬链接只给一个名字）。

### M1 引擎核心
- 实现第 4 节数据结构；`ff-core` 的构建器；`names/names_lower` 大 `memmem` 并行扫描；多词 AND；`write_path`；`ff-cli search`。
- **验收**：
  - 单元测试覆盖：小写副本长度一致性、路径现算（含深层、环检测）、AND 稀有词优先、空查询、Unicode 名字；
  - 合成 200 万条数据基准：子串 ≤ 100ms，内存 ≤ 300MB（若不满足，记录并分析）；
  - 真实卷：建索引时间、条目数记录到 BENCH.md。

### M2 持久化 + 增量
- 第 6 节存取；第 5.3 节 USN 续读；Base + Overlay；后台合并；重建条件。
- **验收**：
  - 重启后载入 ≤ 0.5s；
  - 在被索引卷上创建 / 重命名 / 移动目录 / 删除文件，≤ 1 秒后搜索结果正确（写集成测试脚本，需管理员）；
  - 保存过程中强制结束进程，下次启动仍能载入旧文件或触发重建，**不崩溃**；
  - 手动让 journal id / next_usn 失效，能正确触发全量重建；
  - 合并期间持续搜索，不出现长时间阻塞（记录 p99 延迟）。
- **状态（2026-10-24，`feat/m2-persistence`，见 DECISIONS #58–#61）**：
  - 持久化格式 FFIX 已落地（M2-① `05a9509` + 本轮接线）：零依赖、自 CRC32、原子写、`load` 容错（`None`→重建）。`save_volume`/`save_all`/`load_all` + `set_save_dir` 已接通；`EngineStatus::last_saved` 由 `now_filetime()` 填充。
  - Overlay（Base+Overlay 合并）与后台 `read_changes` 轮询（100ms slice，`MERGE_THRESHOLD=50,000` 触发 `merge_overlay_into_base`）已落地；`overlay_upsert_reflected_in_search` / `overlay_remove_excluded_from_search` 单测覆盖 create/delete 反映。
  - `JournalUnavailable → rebuild_volume` 端到端由 `journal_unavailable_triggers_rescan` 单测覆盖。
  - `engine_persistence_roundtrip` 单测覆盖 FFIX 写→载→`query` 命中一致。
  - **实机项（真实 journal 增量 ≤1s、重启载入 ≤0.5s、强制 kill 后仍载入、合并 p99）需管理员 + 真实 NTFS 卷，不进 CI（决策 26），留待 M2 收尾实机复核**——代码路径已具备，仅差实机计时与 kill 演练。

### M3 能用的 UI（提前，开始自用）
- Tauri 窗口 + 全局热键 + 托盘 + 虚拟滚动列表 + 基本快捷键；`InProcess` 实现的 `EngineClient`。
- **验收**：每次按键到首屏结果 ≤ 50ms（典型查询）；窗口隐藏 / 唤出稳定；连续快速输入没有结果错乱（请求 id 机制）。
- **状态（2026-10-24，main，见 DECISIONS #62）**：
  - `app/src-tauri`（Tauri v2 独立 crate）已落地：6 个引擎命令（`search`/`range`/`cancel`/`status`/`rescan`/`open`）+ 3 个窗口命令（`show_window`/`hide_window`/`toggle_window`）；`setup` 注入 `Arc<InProcessEngine>`；托盘菜单（显示/隐藏/重建索引/退出）；`Ctrl+Alt+Space` 全局快捷键 → `toggle_window`；失焦自动隐藏；`withGlobalTauri: true` 使前端零 npm 依赖直连 IPC。
  - 前端 `TauriEngine.ts` 实现 `EngineClient`（`window.__TAURI__.core.invoke`），`App.tsx` 按 `isTauri()` 在 Tauri/Mock 间切换；`status()` 全链改 `async`（`types.ts`/`MockEngine.ts`/`App.tsx` 轮询同步改）。
  - M3① serde 契约：`ParseError.span`（`{start,end}`）、`ResultRow.name_matches`（`[[start,end],...]`）经 wire 适配器保留 Rust `Range` 类型；枚举 `snake_case`；`VolumeStatus.volume: String`。
  - 引擎接线：`build_engine()` 用 `NtfsVolumeScanner` 构造 `InProcessEngine`，后台线程逐卷 `rescan` 全量扫描不阻塞窗口；非 Windows 走 `empty()`。
  - **未实现/未验证（诚实记录）**：双击 Ctrl 唤出（`WH_KEYBOARD_LL`）留待后续（M3 仅 `Ctrl+Alt+Space`）；`rescan` 粒度粗（忽略盘符参数重建全部，引擎无按盘符查卷 API）；Tauri crate 编译链已在沙箱验证（`cargo check`/`cargo build` 出 `sift-app.exe` 15.2MB、clippy/fmt 全绿），**但运行时 GUI 项未验证**：本沙箱无显示器/WebView2 运行时，无法起窗口，「按键→首屏 ≤50ms」实机计时、窗口隐藏/唤出稳定性、托盘/全局热键实际行为需在有 WebView2 的机器上 `npm run build:tauri` 或 `tauri dev` 实机复核。
  - **2026-11-15 真机补测（DECISIONS #78）**：release `sift-app.exe` 已在
    WinSta0 交互桌面 + WebView2 运行时实机启动（CDP 9222），窗口/输入/
    剪贴板/菜单均真实可操作。**「按键→首屏 ≤50 ms」按端到端口径实测
    145.6 / 146.2 / 146.8 ms，不达标**：120 ms 防抖占主导，引擎仅 6–7 ms
    （防抖后段 ~26 ms 达标）。该验收项与 `UI_SPEC` §7.4 的 120 ms 防抖
    字面互斥，需按 DECISIONS #80 二选一裁定口径，本轮不改代码。
    托盘 / 全局热键 / 失焦隐藏稳定性仍待实机。

### M4 查询语法与属性过滤
- 第 7 节解析器与修饰符；按列排序；Top-K。
- **验收**：语法测试集 ≥ 80 条（含错误输入与半成品输入）；`size:` `dm:` `ext:` `path:` `regex:` 的边界用例；NOT / OR 优先级用例。
- **状态（2026-10-26，main，见 DECISIONS #67 / BENCH M4 节）**：
  - 解析器 AST 化（`Expr::{Empty, And, Or, Not, Name, Pred}`，手写递归下降，NOT > AND > OR）；
    §7.2 的 M4 必做修饰符**全部落地**：`case:` `nocase:` `file:` `folder:` `ext:` `path:` `regex:`
    `ww:`/`wholeword:` `startwith:` `endwith:` `size:` `dm:` `dc:` `da:` `parent:` `infolder:` `depth:` `attrib:`。
  - 语法测试集 `crates/ff-core/tests/m4_syntax.rs` **106 条**（≥80 达标），含错误输入、
    半成品输入容错、各修饰符边界、NOT / OR 优先级，并断言结果恒升序去重、取消无半成品。
  - 按列排序（§7.2）与 Top-K 均已接线（`sort_hits` + `req.max_hits` 截断，M3 已有）。
  - **实测**（C: 880,465 条，release）：`folder:Windows` 3 ms、`ext:pdf` 2 ms、`*.log` 5 ms、
    UI chips 多扩展名 33–46 ms、`size:>500mb` 6 ms、`regex:` 69 ms——均在 §1.3 的 ≤100 ms 内。
  - **已知边界（诚实记录）**：`attrib:` 在索引路径只能判定 R/H/S/D 四个派生位
    （详见 DECISIONS #67）；通配符 / 正则在 M4 不做命中高亮；`dm:today` 等相对日期
    依赖注入的时区偏移，未注入时按 UTC 日界（Windows 侧已注入真实值）。
  - 副作用：本次一并修掉 UI 筛选 chips 全部报解析错误的 P0（`ext:` / `folder:` 现在引擎侧支持）。

### M5 模糊 + 拼音 + 评分
- 第 8 节全部；设置页的规则；高亮范围。
- **验收**：8.2 中的金标准测试全部通过；模糊匹配全表耗时实测并记录；中文用例（首字母、全拼、混合、多音字）通过。
- **状态（2026-11-15，main，见 DECISIONS #68–#73 / BENCH M5 节）**：
  - **T1/T2/T3 分层已接入真实搜索链路**：`ff-core::score::search_ranked`
    （T1 走 M4 AST 求值，T2 拼音只遍历 `cjk_ids`，T3 模糊在 T1+T2 < 100 且
    needle ≥ 2 字符时并行全表）由 `InProcessEngine::search` 调用；命中带
    `tier` + `score`，`SortKey::Score` 按真实评分排序。Overlay 增量条目经
    `match_overlay_name` 走同一套 T2/T3 判定，避免 M4 那次的增量漏匹配重演。
  - **评分**：`ScoreConfig` 权重集中一处（§8.2 公式骨架），§8.2 的
    `prior_folder_bonus` / `rule_bonus` / `frecency_bonus` 按设计留接口、当前为 0。
  - **高亮**：Base 与 Overlay 共用 `ff-core::highlight::highlight_name`
    （T1/T2/T3 区间），引擎在构造 `ResultRow` 时把 UTF-8 字节偏移转成前端
    UTF-16 下标（中文 / emoji 不再错位）。
  - **设置页接线**：`fuzzy` / `pinyin` 直通 `SearchOptions`，单个
    「显示系统/隐藏文件」按 §11.3 成对派生 `hide_system` / `hide_hidden`；
    按 UI_SPEC §4.8.1 摘掉已接线项的「未接线」标注。
  - **验收实测**（200 万条合成索引，release，`bench/src/bin/m5probe.rs`，
    固定种子 0x5349_4654）：T1 子串 2.5 ms、T2 拼音 31.8 ms、
    T3 模糊全表 23.0 ms、评分排序（78,607 命中）110.3 ms —— 均落在
    §8.1 的 150 ms 门槛内。金标准用例（完全相等 > 前缀 > 词首 > 中间 > 模糊、
    命中文件名、浅路径优先、`vsc`→Visual Studio Code、`zgr`→中国人、
    `pysousuo`→拼音搜索Everything）由 `ff-core`/`ff-match`/`ff-engine` 单测覆盖。
  - **真实卷实机复核**（release `ff-cli`、C: 卷 817,903 条）：`wendang` 7 ms
    命中「文档模板.md / 新建文本文档.txt」（T2），同批结果含 T3 模糊命中的长
    中文书名；`wd` / `文档` / `report --sort score` 均 2–4 ms。详见 BENCH M5 节。
  - **M5 后的缺陷收尾轮（同日，DECISIONS #74–#77）**：P0-2 复制路径改为
    Win32 剪贴板 FFI 真实写入（新增 `OpenAction::CopyName`、失败透传、成功 toast）、
    P0-3 结果行四项右键菜单（键盘可达 + 状态栏可发现提示）、P1-1
    `.topbar` 拖拽区解决无边框窗口不能移动。三项均为 M5 完成后的缺陷收尾，
    **不新开里程碑**；`UI_SPEC` §4.2 原标右键菜单 P2/M7 的归属已同步改掉。
  - **门禁**：fmt / clippy `-D warnings` 双 workspace 绿、`cargo test --workspace`
    **211 passed**（收尾轮 +3：宽串编码 / 路径末段 / 真实剪贴板往返）、
    `npm run build`（50 modules）绿；`app/src-tauri` check 绿。
  - **未实现 / 已知边界（诚实记录）**：
    - §8.3 的用户规则（FolderPriority / FileNamePriority / …）、PriorFolders
      与设置页评分规则编辑器**未实现**，界面仍禁用并标注「M5 预留，未实现」；
    - 双拼方案、多音字、生产拼音库 `ib-pinyin` 与 `nucleo-matcher` 未引入
      （用自实现精简字典 + 有序子序列，理由与替换条件见 DECISIONS #70）；
      因此验收项里的「多音字」**未覆盖**；
    - §8.2 的 Top-K 部分选择（`select_nth_unstable_by`）未做，当前对全部命中
      评分后全量排序；78,607 命中实测 110.3 ms，尚未成为瓶颈；
    - M4 的边界仍在：`attrib:` 索引路径只判 R/H/S/D，通配符 / 正则不高亮。
    - **收尾轮三项已于同日 Tauri 真机实测通过**（DECISIONS #78 / BENCH
      「GUI 真机验证」节）：窗口 rect 真实位移 +192,+106；`Ctrl+C` 真实写入
      系统剪贴板并读回完整路径；右键菜单四项 + 键盘 / 鼠标两条执行路径 +
      Esc 语义全部有 DOM 断言与截图（`docs/evidence/`）。
      **同时更正**：真机首测抓到 `start_dragging not allowed by ACL`，
      补 `app/src-tauri/capabilities/default.json` 后拖动才真正生效
      （DECISIONS #79）——仅有 `data-tauri-drag-region` 不充分。
      仍未覆盖：托盘 / 全局热键 / 失焦隐藏稳定性 / 中文 IME 组词。

### M6 服务与开放接口
- 第 10、12 节：服务拆分、命名管道、HTTP API（含全部安全要求）。
- **验收**：UI 以普通权限运行即可搜索；HTTP 用错误 token / 错误 Host 头请求会被拒绝（写测试）；服务崩溃后 UI 能提示并重连。

### M7 进阶（逐项单独立项）
对话框集成（`SetWinEventHook` + UI Automation，先在记事本"打开"对话框验证）· 内容搜索 · 硬链接多路径 · filelist 导入 · 非 NTFS 回退遍历 · 属性窗口。（**结果行右键菜单已于 M5 收尾轮提前实现**，见 §13-M5 与 `UI_SPEC.md` §4.2，不再列在 M7。）

---

## 14. 测试与基准

- **合成数据生成器**（`bench/`）：可指定条目数、目录深度分布、CJK 名字占比、重名率，固定随机种子；用来复现性能数字。
- `ff-core` / `ff-match`：纯单元测试 + `proptest`（可选）验证路径生成与查询求值不 panic。
- 集成测试（需要管理员与真实 NTFS 卷）放在 `tests/windows_integration/`，默认 `#[ignore]`，通过 `cargo test -- --ignored` 手动运行；使用临时目录创建文件来验证 USN。
- `criterion` 基准覆盖：建索引、载入、子串、AND、拼音、模糊、Top-K 排序、按列排序。
- 每个里程碑的结果追加到 `docs/BENCH.md`，格式：日期 · 机器（CPU/内存/磁盘）· 条目数 · 指标 · 数值。

---

## 15. 已知风险与注意事项

| 风险 | 对策 |
|---|---|
| 需要管理员权限（UAC） | M0–M5 管理员运行；M6 服务化 |
| 杀毒软件对"直接读卷"敏感 | 不做注入、不写系统目录；记录并在文档说明，必要时提供数字签名 |
| ReFS / V3 USN 记录 | 明确不支持，遇到记录日志并忽略 |
| 硬链接（一个文件多个名字） | `ntfs-reader` 可能只给一个名字；首版接受，M7 评估 |
| 日志被截断 / 卷被重新格式化 | 第 5.3 节"重建条件" |
| 路径超长（> 260） | 内部一律使用 UTF-8 字符串；打开文件时用 `\\?\` 前缀 |
| 大小写与 Unicode 折叠 | 默认简单小写；土耳其语 `İ` 等特例有测试，不做完整 Unicode case folding |
| 内存超标 | 先量；必要时把 `names` 与 `names_lower` 之外的冷列（atime / ctime）延迟加载 |
| 损坏的索引文件 | 校验失败 = 重建，**绝不 panic** |
| 本地 HTTP 泄露文件列表 | 第 10.2 节全部安全要求为硬性 |
| 许可证 | 引入依赖时核对许可证并写入 `docs/DEPENDENCIES.md`；`nucleo` 系列的许可证条款需特别留意并记录 |

---

## 16. 待我确认的开放项（AI 遇到时请直接问我）

1. 产品最终名称、图标。
2. 旧版 FlashFind 的现有代码：是否复用其中的索引部分（我会在需要时提供）。
3. 默认全局热键最终值。
4. 是否需要兼容 Everything 的 IPC / HTTP 接口（影响现有第三方工具）。

---

## 附：给开发 AI 的第一条指令（直接复制）

> 请完整阅读 `docs/DESIGN.md`，然后**只做 M0**。
> 做完后把实测结果写入 `docs/BENCH.md`，并列出 `ntfs-reader` 的接口与限制（版本号、能提供的字段、需要的权限、已发现的坑）。
> 不要开始 M1。汇报时先给结论，再给数据。
