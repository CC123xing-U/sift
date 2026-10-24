# Listary 6.3.5.94 功能实现文档（重写用）

> 用途：交给 AI 重写一个功能等价的程序。只描述"功能怎么实现"，不含分析过程。
> 目标平台：Windows x64。参考实现：Listary.exe（.NET/WPF）+ listary_engine.dll（Rust）+ HookHost×2 + Service。

---

## 1. 进程架构（五进程）

```
Listary.exe           .NET WPF 前端：启动器/文件搜索窗口/选项、热键响应、搜索编排
Listary.Service.exe   .NET Windows 服务：提权常驻，承载引擎（索引需要读卷权限）
ListaryHookHost64/32  原生宿主：InstallHook/RemoveHook/HookShellWindow，
                       加载 ListaryHook64-6.1.10.1.dll（.NET 混合模式钩子）注入目标进程
listary_engine.dll    Rust 引擎：索引 + 搜索 + DB（可被宿主进程内加载，也可被服务承载）
FileAppPlugin.Host    .NET：第三方文件管理器插件宿主（DirectoryOpus 等）
```

进程间通信三件套：
- **共享内存** `ListaryX_MainSharedMemory`（UI 与 HookHost 之间传搜索词/结果）
- **注册窗口消息** `WM_LISTARY_SENDMESSAGE`（唤醒/通知）
- **gRPC**（UI/服务 ↔ 引擎，跨进程/跨机）
- 单实例：`ListaryHookHost64Mutex`；双击 Ctrl 监听窗口类 `ListaryDoubleCtrlMonitorClass`

---

## 2. Rust 引擎（listary_engine.dll）——核心

### 2.1 模块划分

```
listary-core/src/
├── ffi/mod.rs                    UniFFI 接口层
├── indexer/
│   ├── mod.rs
│   ├── folder/mod.rs             自定义文件夹索引（遍历用户目录）
│   └── ntfs/
│       ├── mft_reader.rs         直接解析 $MFT::$DATA
│       └── list_ntfs_volumes.rs  枚举 NTFS 卷
├── db/
│   ├── file_db.rs                主 DB（folder tree + 文件节点 + 硬链接槽）
│   ├── read_only.rs              只读视图
│   ├── snapshot.rs               快照（一致性读）
│   └── priority.rs               优先级库（搜索排序规则）
├── indexed_search/
│   ├── service.rs                gRPC 服务实现
│   ├── score_based_searcher.rs   评分搜索器
│   └── path_filter.rs            路径过滤
├── string_matcher/
│   ├── bm_searcher.rs            Boyer-Moore 子串
│   ├── keyword_string_matcher.rs 多关键字
│   ├── fuzzy_string_matcher.rs   模糊匹配
│   ├── pinyin_searcher.rs        拼音（全拼/首字母）
│   └── fuzzy_pinyin_searcher.rs  模糊拼音
├── fuzzy/position_bitmap.rs      模糊匹配位置位图加速
├── pinyin/mod.rs                 拼音数据
└── utils/{string.rs, chrono.rs}
```

运行时：**tokio 1.46.1**（多线程调度 + mpsc 消息通道 + rwlock）；数据库 **rust-rocksdb**；哈希表 **me-hashbrown**（hashbrown fork）；序列化/传输 **tonic + prost**；压缩 bzip2。

### 2.2 索引生成

**全量（首次建索引）——MFT 直读：**
```
1. 枚举卷（FindFirstVolume/FindNextVolume）取 \\?\Volume{GUID}
2. CreateFileW("\\\\.\C:") 打开卷
3. 读 $MFT 的 $DATA 数据流（先解析 MFT 记录 0 拿 data run 列表，再顺序读整个 $MFT）
4. 逐 MFT 记录解析：
   - 记录头 + 属性（$STANDARD_INFORMATION: 时间/属性；$FILE_NAME: 名字+父FRN；
     $DATA: 大小/run 列表；$INDEX_ROOT 有则目录）
   - FRN = 记录号；父 FRN 来自 $FILE_NAME
   - 名字按父 FRN 挂 folder tree
5. 硬链接：同文件多 $FILE_NAME → 一个 id + 多个硬链接槽
```

**增量——USN Journal：**
```
FSCTL_QUERY_USN_JOURNAL 取 FirstUsn/NextUsn/journal id
FSCTL_READ_USN_JOURNAL 从保存的 next_usn 续读变更
卷变更（next_usn 回退）= journal 被删重建 → 忽略并全量重建
```

**自定义索引：** 遍历用户目录（folder/mod.rs），由优先级规则驱动。

**落库：** `db::builder` 批量写入 RocksDB；日志级诊断（格式版本当前 4，不兼容直接重建）。

### 2.3 数据库：RocksDB（LSM 树）

```
位置: %APPDATA%\Listary\UserProfile\Cache\<主机名>\Index\
文件: 0000NN.log(WAL) / 0000NN.sst(SSTable) / CURRENT / MANIFEST-* / IDENTITY /
      LOCK / OPTIONS-* / LOG / LOG.old.*
列族/键前缀: "db_path" + "indexers"（引擎内字符串）
结构: folder tree 节点（节点 id）+ 文件节点 + 硬链接槽
一致性: snapshot.rs 用 RocksDB 快照 → 写不阻塞读
优先级: priority.rs 独立库，存 FolderPriority/FileNamePriority/SearchFilter 规则
实测规模: C 盘 ~81MB（48MB WAL + 32MB SST，重建中）
```

重写替代方案：嵌入式 KV（LevelDB/RocksDB）或自研 LSM + 快照；核心是不阻塞读写。

### 2.4 搜索与匹配

**五种匹配器（按查询特征选用/组合）：**

| 匹配器 | 算法 | 场景 |
|---|---|---|
| bm_searcher | Boyer-Moore | 连续子串 |
| keyword_string_matcher | 多词 AND | "red fox" |
| fuzzy_string_matcher + position_bitmap | 字符序匹配 + 位置位图剪枝 | "vsc"→"Visual Studio Code" |
| pinyin_searcher | 全拼/首字母映射 | 中文文件名（数据 cnmatch.bin 60KB） |
| fuzzy_pinyin_searcher | 拼音容错 | 声母/韵母模糊 |

**评分排序（score_based_searcher）：**
```
score = f(匹配质量, 上下文, 规则)
- 匹配质量：命中位置（越靠前越高）、是否连续、是否全词、大小写一致
- 上下文：PriorFolders（最近访问目录加权，InPriorFolder 加分）
- 规则：FolderPriority / FileNamePriority（正则）/ FileExtensionPriority /
        FileAttributePriority（来自 priority.rs，可 gRPC 热更新）
- IsProSearchResult / CombinedScore 合成最终分
```

**搜索参数（PathDbSearchParam）：**
```
MaxResults / StringMatcher / UseMatchScore / CancellationToken /
PriorFolders / OnlySearchInPriorFolders / SearchFiles / SearchFolders /
SearchLowPriorityFiles / FolderPartMatchers / OnlyMatchBaseName
```

---

## 3. 对外接口（三通道）

### 3.1 UniFFI P/Invoke（进程内，引擎 106 个导出）

```csharp
[DllImport("listary_engine")]  // CallingConvention.Cdecl
```

```
uniffi_listary_engine_fn_constructor_listaryengine_new
uniffi_listary_engine_fn_clone_listaryengine / fn_free_listaryengine
fn_method_listaryengine_{
    start, shutdown, restart, echo,
    add_indexes, remove_indexes, rebuild_indexer, wait_for_indexers, poll_indexer,
    get_indexer_infos, get_indexer_diagnostics, get_process_info,
    open_session, release_session, search, get_search_results, cancel_request,
    test_error, test_panic, test_server_error }
ffi_listary_engine_rustbuffer_alloc
ffi_listary_engine_rust_future_*（全类型 async Future：cancel/complete/free × i8..u64/pointer/void）
UNIFFI_META_NAMESPACE_LISTARY_ENGINE / UNIFFI_META_UDL_LISTARY_ENGINE
```

搜索是**异步**的（Task 返回），可取消（cancel_request + CancellationToken）。

### 3.2 gRPC：IndexedSearch 服务

```
service IndexedSearch:
    OpenSession(IndexedSearchStartParams) -> OpenSessionResult
    Search(IndexedSearchSearchParams) -> SearchResultsSummary      // 两段式第一段：总数+摘要
    GetSearchResults(IndexedSearchGetSearchResultsParams) -> SearchResultDetails  // 第二段：offset/limit 详情
    AddIndexes / RemoveIndexes / RebuildIndexer / PollIndexer / WaitForIndexers
    GetIndexerInfos / GetIndexerDiagnostics / GetProcessInfo / CancelRequest / Restart / Shutdown / Start / Echo
```

消息类型（32 个，重点）：
```
IndexedSearchSearchParams, SearchResultsSummary, SearchResultDetail(s),
IndexedSearchGetSearchResultsParams, IndexedSearchStartParams, IndexedSearchIndexes,
IndexerInfo(s), IndexerDiagnostics(Params),
FolderPriorityRule, FileNameRegexPriorityRule, FileExtensionPriorityRule, FileAttributePriorityRule
```

### 3.3 共享内存 + 窗口消息（UI ↔ HookHost）

共享内存块 `ListaryX_MainSharedMemory` 传结构化数据；`WM_LISTARY_SENDMESSAGE` 唤醒对方；互斥体保证单实例。

---

## 4. .NET 前端（Listary.exe）

### 4.1 应用骨架

- `App : Application` / `Main()` / `OnStartup`
- 窗口：启动器（Launcher）、文件搜索窗口（FileSearchWindow）、选项（Options）
- WPF 栈：HandyControl + MahApps.Metro + ControlzEx + GongSolutions.WPF.DragDrop + XamlAnimatedGif + WebView2
- DI：Autofac；日志：NLog；遥测：Sentry + RudderAnalytics；动态表达式：DynamicExpresso；自动更新：AutoUpdater.NET

### 4.2 启动器 = 多提供器搜索（核心设计）

```
interface ISearchSession {
    Task<List<ISearchItem>> Search(SearchQuery, CancellationToken, ISearchSession);
}
```

用户输入同时路由到多个提供器，各自返回 ISearchItem，合并后按分数排序：
- 索引文件搜索（IndexedSearch，引擎）
- 当前打开文件夹/标签（MenuCurrentlyOpenedFolders）
- 收藏夹（Menus/Favorites）
- 命令（Commands）
- 动作（Actions）
- 网页搜索（WebSearch，可配模板）
- 计算器（CalcBinding，DynamicExpresso 求值）

### 4.3 配置：Preferences.json（CRDT 变更集）

```json
{
  "AppConfigs":    { "TypeConfigs": { "DirectoryOpus": {} }, "PathConfigs": {} },
  "Menus":         { "Favorites": { "Deletions":[], "Moves":[], "Insertions":[], "Updates":[] } },
  "DiskSearch":    { "FolderPriority": {…}, "FileNamePriority": {…}, "SearchFilter": {…} },
  "FileSearchWindow": {
    "Columns": [ {"Id":"FileName","Width":200}, {"Id":"Path","Width":400}, {"Id":"DateModified","Width":200} ],
    "WindowWidth": 1000, "WindowHeight": 720, "PreviewWidth": 300, "SideBarWidth": 225 },
  "Commands": { "Items": {…} }, "WebSearch": { "Items": {…} }, "Actions": { "Items": {…} },
  "AutoUpdate": {},
  "Settings": { "General.LauncherPositionX": 401, "General.LauncherPositionY": 29,
                "Tutorial.ShowMainV3": false, "Listary5.SettingsImported": true },
  "Extensions": { "Extensions": { "<guid>": {} } }
}
```

每个块都是 `{Deletions, Moves, Insertions, Updates}` 变更集——**为多设备同步设计**（同步的是操作，不是全量状态）。另有 `PathHistory.json`、`SearchHistory.json`。

---

## 5. Hook 与系统集成（Listary 的招牌）

### 5.1 钩子体系

```
ListaryHookHost64/32.exe（常驻，无窗口或隐藏窗口）
  ├─ InstallHook / RemoveHook / CreateHookHelper / HookShellWindow
  ├─ 加载 ListaryHook64-6.1.10.1.dll（.NET 混合模式，故宿主引用 mscoree）
  ├─ 注入目标进程（Explorer / 文件对话框 / Total Command / DirectoryOpus…）
  └─ 通过共享内存 + WM_LISTARY_SENDMESSAGE 与 UI 进程通信
```

挂钩点：`ext-ms-win-ntuser-dialogbox-*`（**文件打开/保存对话框**——任意对话框里唤出 Listary 搜索并跳转）、Shell 窗口、非客户区点击（NCXBUTTONDBLCLK 等）。

### 5.2 双击 Ctrl 唤出

独立窗口类 `ListaryDoubleCtrlMonitorClass` 全局监听双击 Ctrl（避免注册热键冲突与被占用）。

### 5.3 FileAppPlugin 协议（第三方文件管理器集成）

```
IFileAppPlugin / IFileAppPluginHost
IFileWindow / IFileTab / IGetFolder / IGetFileTabs
IOpenFile / IOpenFolder / IOpenFolderAndSelectFile
CanWindowOpenFolder / CanTabOpenFolder / CanOpenFolderAndSelectFile / IsOpenedFolderProvider
SearchBarType
```

### 5.4 Shell 互操作（P/Invoke 面，约 1000 字段）

IShellFolder / IShellItem / IEnumIDList / IEnumShellItems / STRRET / IPropertyStore / PROPERTYKEY / SFGAO / SIGDN / SHGDN / SHCONT + SHOpenFolderAndSelectItems + SHGetKnownFolderItem + DwmWindowAttribute + ACL（FileProtection/FileRights）+ CreateRemoteFileDelegate（跨进程远程创建文件）。

---

## 6. 数据文件全清单

```
%APPDATA%\Listary\UserProfile\
├── Cache\<主机名>\Index\        RocksDB（WAL + SST + MANIFEST/OPTIONS/CURRENT/LOCK/IDENTITY/LOG）
├── Cache\<主机名>\ListaryEngineLog.txt    引擎日志（tracing）
├── Cache\<主机名>\ListaryMiniCoreLog.txt
├── Settings\Preferences.json   主配置（CRDT）
├── Settings\PathHistory.json   路径历史
└── Settings\SearchHistory.json 搜索历史

Program Files (x86)\Listary\
├── Listary.exe / Listary.Service.exe / ListaryHookHost32/64.exe
├── listary_engine.dll（Rust）
├── Listary.Interop.dll / Listary.Common.dll / Listary.Localization.dll
├── Listary.FileAppPlugin.dll / .Host.dll / .DevTools.exe
├── Hooks\ListaryHook64-6.1.10.1.dll
├── FileAppPlugins\ / runtimes\ / Images\ / Tutorial\
├── cnmatch.bin（拼音数据 60KB）
└── 40+ 语言文件夹（zh-Hans/zh-Hant/ja/de/fr…）
```

---

## 7. UI 参数（文件搜索窗口）

```
列: FileName(200) / Path(400) / DateModified(200)，可增删改宽
窗口: 1000×720；预览面板 300；侧栏 225
```

---

## 8. 重写要点

### 8.1 最小充分集（按优先级）

1. **索引**：MFT 直读全量 + USN 增量 + KV 存储（LSM/自研）+ 读写分离（快照/双缓冲）
2. **搜索**：子串（BM 或 bytes.find）+ 模糊（位置位图）+ 拼音（全拼/首字母，可先用现成拼音库）+ 评分排序
3. **评分**：命中位置 + 连续度 + PriorFolders 上下文 + 用户优先级规则
4. **入口**：UI（先做窗口内搜索框）+ 一种 IPC（共享内存或 HTTP）
5. **场景集成**（后期）：对话框钩子 + 双击 Ctrl

### 8.2 关键设计决策（照抄不亏）

- folder tree + 文件节点 + 硬链接槽（去重、省空间）
- 两段式搜索（先摘要后详情）= 低延迟首屏
- CRDT 变更集配置 = 天然可同步
- 多提供器启动器 = 一个框搜一切
- 写索引不阻塞搜索（快照）
- 引擎与 UI 分离 + 服务提权 = 低权限 UI 也能索引全盘

### 8.3 依赖替代（重写时）

| 原依赖 | 替代 |
|---|---|
| RocksDB | LevelDB / 自研 LSM / SQLite(WAL) |
| tokio | 线程池 + 任务队列 |
| tonic/prost | HTTP/JSON、gRPC、或具名管道 |
| me-hashbrown | 标准库 HashMap/FxHashMap |
| cnmatch.bin | 开源拼音库（如 pypinyin 数据 / rust-pinyin） |
