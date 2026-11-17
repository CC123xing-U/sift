# 02 · 源码地图与逐文件复核清单

> **本文件两种用法**：① 交接时快速理解每个模块在哪；② **动手前按第 12 节清单重新打开每个相关源文件**。
> 行号来自 2026-11-17 的磁盘内容，改动后会漂移——请把它当线索，不当定论。
> 这张地图由逐文件审阅源码生成，但**仍可能遗漏**——这正是要求你复核的原因。

## 1. 依赖拓扑（从各 Cargo.toml 实证）

```
ff-match ─────────────┐
ff-core ──(ff-match)──┤
ff-ntfs (独立,不依赖 core) ├─ ff-engine ──┬─ ff-ipc ── ff-service
                       │              ├─ ff-http ─┘
                       └──────────────┴─ ff-cli
ff-dialog (独立)
app/src-tauri (独立 workspace) 依赖 6 个 ff-* crate
```

- 根 workspace 成员：`Cargo.toml:8-22`（9 个 ff-* + bench + tests）。
- 统一 `version=0.1.0 / edition=2021 / publish=false`。
- 平台隔离统一 `#[cfg(windows)]`；ff-core / ff-match 声明 `#![forbid(unsafe_code)]`。

## 2. ff-core（纯逻辑内核，forbid unsafe）

职责：SoA 条目表、路径现算、查询解析/AST 求值、分层匹配评分、内容搜索、filelist 导入、FFIX 持久化。

| 文件 | 职责 | 关键 API（file:line） |
|---|---|---|
| `src/lib.rs` | crate 根，12 个 pub mod + ParseError 契约 | `ParseError:88` |
| `src/cancel.rs` | 协作取消令牌（Arc\<AtomicBool\>） | `CancelToken:16` |
| `src/case.rs` | 逐字节小写折叠、CJK 检测 | `simple_lowercase:10` `has_cjk:53` |
| `src/entry.rs` | 条目表核心、IndexBuilder、VolumeIndex | `EntryId:11` `IndexBuilder:115` `VolumeIndex:274` |
| `src/path.rs` | 沿 parent 链现算完整路径（MAX_CHAIN=512） | `path_string` `write_path` |
| `src/eval.rs` | 无索引位置的 AST 判定（供 Overlay） | `EntryFacts:16` `matches_facts:35` |
| `src/highlight.rs` | 名字高亮区间（UTF-8 字节偏移） | `highlight_name:19` |
| `src/content.rs` | 内容搜索：Matcher/ContentFilter/ContentSource | `MAX_FILE_BYTES:33` `ContentSpec:54` `scan_stream:191` `ContentFilter:294` |
| `src/import.rs` | filelist 纯路径/CSV 解析、按盘分卷建索引 | `parse_list:89` `normalize_path:312` `build_indexes:463` |
| `src/persist.rs` | FFIX 格式（magic FFIX, version 1）+ CRC32 | `FORMAT_VERSION:42` `save:218` `load:245` |
| `src/query.rs` | 查询语言 M4 全套 AST（~1268 行） | `parse:1005` `Expr:181` `win32_attr:379` |
| `src/score.rs` | M5 分层调度与评分 | `match_tokens:118` `search_ranked:261` |
| `src/search.rs` | 求值主循环（memmem 一次大扫 + 候选校验） | `VolumeIndex::search:86` `depth_of:430` |

测试：内联单测集中在 search/entry/import/highlight/path/content/query 等（约 100+）；
集成测试 `tests/m4_syntax.rs`（M4 语法表驱动）、`tests/api_m1.rs`（含 1 个 ignored 性能冒烟）。

## 3. ff-match（匹配器，forbid unsafe）

| 文件 | 职责 | 关键 API |
|---|---|---|
| `src/lib.rs` | 再导出 Tier/Matcher/三类匹配器/scan_names | — |
| `src/matcher.rs` | Matcher trait + 子串(memchr) + 模糊(字符序子序列) | `Tier:12` `Matcher:27` `SubstringMatcher:40` `FuzzyMatcher:88` |
| `src/pinyin.rs` | 精简汉字→全拼/首字母字典；`\u{ffff}` 作子序列屏障 | `pinyin_of:14` `PinyinTokens:81` `PinyinMatcher:138` |
| `src/scan.rs` | 名字 arena 按 0x00 切块交 rayon 并行扫描 | `scan_names:24` |

依赖：memchr、rayon。**T2/T3 是演示级**（多音字/双语未覆盖），替换条件见 DECISIONS #70。

## 4. ff-ntfs（NTFS 读取，不依赖 ff-core）

| 文件 | 职责 | 关键 API |
|---|---|---|
| `src/lib.rs` | 冻结契约类型 + VolumeScanner trait | `VolumeId:18` `RawEntry:39` `Change:60` `ScanMeta:69` `ScanError:83` `VolumeScanner:133` |
| `src/usn.rs` | USN 纯逻辑（V2 解析、reason 合并、Win32 错误分派）~899 行 | `parse_usn_journal_buffer:150` `classify_read_outcome:57` `records_to_changes:375` |
| `src/walk.rs` | 非 NTFS 目录遍历回退（read_dir 手写，不跟随链接） | `DirWalkScanner:44` `path_key:231` |
| `src/scanner.rs` | NTFS 实现（MFT 全扫 + USN 增量 + 硬链接别名产出） | `NtfsVolumeScanner:46` `device_path_for:234` |
| `src/auto.rs` | 按 fs_name 分派 NTFS/遍历，先试 NTFS 失败回退 | `AutoVolumeScanner:24` |
| `src/win32.rs` | Win32 封装：卷枚举/USN Journal/内存/时区（cfg windows） | `enumerate_volumes:118` `read_usn_journal:339` `create_usn_journal:447` |

测试：usn.rs 纯逻辑单测（~20，主力）、walk/scanner/win32/auto 内联；工作区级
`tests/tests/windows_integration.rs`（~5，需管理员+真卷，全 `#[ignore]`）。
注意 `ff-ntfs` 输出中立 `RawEntry`/`Change`，由 ff-engine 映射到内部 id。

## 5. ff-engine（契约层 + InProcess 实现，最大 crate）

| 文件 | 职责 | 关键 API |
|---|---|---|
| `src/lib.rs` | crate 根 + 再导出（**顶部注释滞后，见 05-KNOWN-ISSUES #1**） | — |
| `src/api.rs` | 对外冻结契约 EngineClient | `EngineClient:252`（search:256/range:259/cancel:262/status:265/rescan:268/open:271） `SearchRequest:116` `SearchSummary:127` `OpenAction:165` |
| `src/inprocess.rs` | 核心实现（~3172 行）：卷管理/Base+Overlay/后台 USN 轮询/落盘/会话 TTL/FFI | `InProcessEngine:885` `create_session:949` `save_all:959` `load_all:964` `EngineSession:1002` `poll_volume`(约:627) |
| `src/overlay.rs` | 增量层：按 Change 合并、父链先 Overlay 再 Base | `OverlayEntry:13` `Overlay:67` `apply:82` |
| `src/ipc.rs` | 共享管道协议：版本化帧、命令/回复、有界 JSON 编解码 | `Message:22` `Command:53` `Reply:102` `serve_connection:176` `MAX_FRAME_BYTES:17` |
| `src/timeconv.rs` | IPC/JS 边界 FILETIME→epoch ms（防精度丢失） | `filetime_to_epoch_ms:38` `convert_rows:51` |
| `src/actions.rs` | UI 进程侧 open 动作执行 | `full_path:15` `execute_open_checked:42` |

测试：inprocess.rs 内联 ~44 个（增量/Journal 重建/AB-BA 死锁回归:2792）；`tests/ipc_contract.rs` ~12。
**改动锁逻辑前务必读 inprocess.rs:620-626 的 AB-BA 死锁注释与回归测试 2792-2802。**

## 6. ff-dialog / ff-ipc / ff-http / ff-service / ff-cli

- **ff-dialog**（M7）：`src/detect.rs` 纯逻辑分类（`classify:34`，标题多语言关键词）；`src/win.rs`
  SetWinEventHook + UIA `ValuePattern::SetValue` 回填（`DialogWatcher:159` `fill_active_dialog:289`，手写 user32 extern）。
  测试仅 detect.rs（~6），win 靠真机。
- **ff-ipc**（M6，整 crate cfg(windows)）：`src/pipe.rs` 管道 HANDLE 包装 + SDDL ACL（`PipeListener:137`）；
  `src/sid.rs` SID/SDDL（`pipe_name:91`）；`src/client.rs` `PipeClient:24`（断线识别 `is_disconnected:50`）；
  `src/server.rs` `serve_forever:31`（每连接独立 session + 线程）。测试 `tests/pipe_roundtrip.rs` ~3（需真管道）。
- **ff-http**（M6，默认关闭）：`src/security.rs` 安全边界（`check_host:26` `check_bearer:42` `RateLimiter:71`）；
  `src/wire.rs` HTTP 解析 + DTO（`parse_search_query:71`）；`src/server.rs` 只绑 127.0.0.1、无 CORS（`serve_forever:39`）；
  `src/fileconfig.rs` http.json 开关。测试 `tests/http_e2e.rs` ~9（真实 TCP）。
- **ff-service**（M6，cfg(windows)）：单文件 `src/main.rs`（313 行），SCM install/uninstall/start/stop/status +
  run_service:154 + spawn_initial_scan:294。无单测。
- **ff-cli**：单文件 `src/main.rs`（715 行），`scan`/`search`/`import` 三子命令（`run_scan:69` `run_search:298`
  `run_import:406`）。`bench`/`dump` 未实现。测试为参数解析（~7）+ windows 条件测试（~2）。

## 7. app/src-tauri（Tauri 2 后端，仅 main.rs 一个源文件）

`src/main.rs`（~800 行）承载全部后端逻辑。**先读文件头 1-8 行与 780-792 的 handler 注册。**

- **11 个 command**（注册 `main.rs:780-792`）：
  - 引擎六：`search:192`（async+spawn_blocking）、`range:203`（convert_rows）、`cancel:221`、`status:226`、
    `rescan:234`（忽略 volume 统一重建）、`open:249`。
  - 对话框二：`fill_active_dialog:317`、`has_active_dialog:335`（**前端无调用点，见 05 #12**）。
  - 窗口三：`show_window:353`、`hide_window:364`、`toggle_window:392`。
- **emit 事件**：`sift:service-status:159`、`sift:dialog-detected:298`、`sift:dialog-closed:307`、
  `sift:shown:360`、`sift:hiding:368`。Rust 侧**无 listen**。
- **Backend 抽象**：`enum Backend:34`（InProcess/Pipe）；门面 `impl:50-114`；
  `PipeBackend:42` + `with_reconnect:116`（1s 退避重连）；`build_backend:626`（试管道失败回退）。
- **启动流程**：`run():691-795`（build_backend→快捷键→setup→窗口事件→handler）。
  `build_engine:485` 把重活（枚举/清理/load_all/逐卷 rescan）都放后台线程。
- **窗口管理**：隐藏代数 `HIDE_GEN:348`（`gen+1` 判定:374）；`WINDOW_OUT_MS=90:351`；
  失焦自动隐藏三重守卫 `:747-779`；`foreground_pid:416`（重试 3×30ms）。
- **托盘** `build_tray:655`；**调试日志** `dbg_log:445`（待清理）；**热键** `CmdOrCtrl+Alt+Space:699`。

配置：`tauri.conf.json`（窗口 760×480/无边框/透明/alwaysOnTop，`withGlobalTauri:true`，CSP null）；
`capabilities/default.json`（放行 start-dragging + event listen/unlisten；新增 core API 时须同步）。

## 8. app/src（React 前端，15 个源文件）

- `main.tsx:7` 挂 react root；`App.tsx`（575 行）是状态中枢：环境选引擎 `:60`、状态轮询 `:158`、
  防抖搜索 `:269`（120ms）、全局键盘 `:364-491`、事件监听 `:231-267`。
- 组件：`TopBar.tsx`（搜索框/chips/排序）、`ResultArea.tsx`（603 行，虚拟化列表/列头排序/右键菜单 6 项/
  forwardRef）、`StatusBar.tsx`（状态/toast/内容搜索 note）、`Settings.tsx`（367 行，**11 项 pending 全禁用**）。
- `src/engine/TauriEngine.ts:34` 自写 invoke 映射 6 方法；`MockEngine.ts` 浏览器原型（30 万合成数据，open 静默 no-op）。
- `src/types.ts` 镜像 Rust 契约（**与 api.rs 同步改**）；`src/i18n.ts` 仅中文（英文资源不存在）；
  `src/engine/query.ts` 前端另有一套查询解析（与 Rust 引擎需同步，概念漂移风险）。
- 事件监听全在 `App.tsx:231-267`（shown/hiding/service-status/dialog-detected/dialog-closed）。
- 脚本（`package.json:6-15`）：build=tsc -b && vite build；test:format=零依赖格式断言；
  test:ui=需外部 Playwright，仅布局仿真。

## 9. bench / tests

- `bench/src/lib.rs` 合成数据生成器（固定 seed `0x5349_4654`="SIFT"）；`bin/memprobe.rs`、`bin/m5probe.rs`
  实测二进制；`benches/*.rs` criterion 基准。
- `tests/tests/m1_core.rs` 跨 crate 核心验收；`tests/tests/windows_integration.rs` 需管理员真卷（ignored）。

## 10. 关键数据流（理解用，动手时以源码为准）

1. **启动**：Tauri run → build_backend（试管道/回退 InProcess）→ setup 注入状态 + 托盘 + 对话框监听 →
   后台线程枚举卷/清理旧索引/load_all/逐卷 rescan → 窗口显示（失焦守卫防误藏）。
2. **搜索**：前端 120ms 防抖 → invoke search → Backend::search（async spawn_blocking）→
   引擎 search_ranked（T1+T2+T3 + 查询 AST + 评分）→ 若含 content: 串行扫候选文件 →
   SearchSummary → 前端只拿摘要；行数据由虚拟化滚动 debounce 90ms 后 invoke range 拉取（convert_rows 转时间）。
3. **增量**：InProcess 后台线程每卷 poll_volume → read_changes（USN V2）→ Overlay::apply（空名按 frn 回填）→
   增量并入结果；退出时 Overlay 合并进 Base 落盘。
4. **服务模式**：普通权限 UI → PipeClient → 服务（SYSTEM）持引擎跑扫描 → 搜索走管道；
   open 动作服务只返回 ResolvedAction → UI 以当前用户身份 execute_open_checked。

## 11. 不要回退的修复（改动相关文件时先确认）

| 修复 | 位置 | 不回退原因 |
|---|---|---|
| overlay 空名按 frn 从 Base 回填 | `ff-engine/overlay.rs::apply` | USN 记录无名字，否则空名遮蔽命中并污染落盘索引 |
| 窗口隐藏代数 `gen+1` | `app/src-tauri/src/main.rs:374` | 曾写成 `==gen` 致延迟隐藏 100% 被自己作废（#83） |
| 搜索 async + spawn_blocking | `main.rs:186-190` | content: 读盘阻塞 IPC 线程会致窗口「未响应」 |
| 失焦守卫 `fg_pid==0` 不隐藏 | `main.rs:763` | 前台瞬态 NULL 曾致启动 2.7s 即误藏（#96） |
| 重活移入后台线程 | `main.rs:503-553` | 同步 load_all(~5s) 占住窗口创建致延迟显示（#96） |
| start-dragging ACL | `capabilities/default.json` | 仅 data-tauri-drag-region 不足，真机报 not allowed（#79） |
| AB-BA 死锁三阶段分离 | `inprocess.rs:620-626` | volumes.read 与 scanner 锁互抢曾死锁，有 8s 超时回归测试 |
| 属性用 ShellExecuteExW+INVOKEIDLIST | `ff-engine/actions.rs`（Properties 分支） | ShellExecuteW("properties") 返回 31 误报（#91） |
| 对话框回填用 UIA 非 WM_SETTEXT | `ff-dialog/win.rs` | 裸 WM_SETTEXT 对 IFileDialog 无效（#92） |
| 大文件按 metadata 体积一律跳过 | `ff-core/content.rs` | 曾出现开头命中/尾部不命中不一致 |

## 12. 逐文件复核清单（接手必做）

按你要动的模块，把相关源文件**重新打开**确认现状，不要依赖上面的行号或 docs 描述：

- [ ] `crates/ff-engine/src/lib.rs` 顶部注释 vs `inprocess.rs` 实现是否一致（已知矛盾，05 #1）
- [ ] `crates/ff-core/src/query.rs` 里 content/dupe/runcount/len 相关注释是否已落后于实现
- [ ] 你要改的每个 crate 的 `src/*.rs`（按上面 2–9 节的表逐个打开）
- [ ] `app/src-tauri/src/main.rs` 的 command 注册区（780-792）与前端 `TauriEngine.ts` 的 invoke 映射是否一一对应
- [ ] `app/src/components/Settings.tsx` 每个 pending 项的现状（03 T1 会逐条列）
- [ ] `app/src/types.ts` 与 `crates/ff-engine/src/api.rs` 的字段是否同步
- [ ] `Cargo.toml` 两个 workspace 的依赖与 `docs/DEPENDENCIES.md` 是否一致
- [ ] `docs/DESIGN.md` / `docs/DECISIONS.md` 中与你任务相关的章节（确认设计意图，不是照抄）

复核结果若发现新的文档/源码冲突，追加到 `05-KNOWN-ISSUES.md`，并顺手修正本包对应描述。
