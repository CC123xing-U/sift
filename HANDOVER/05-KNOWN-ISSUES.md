# 05 · 已知问题点与源码实证

> 以下均由逐文件审阅源码得出（2026-11-17），带 file:line。**接手 AI 请逐条复核**——
> 有些是「有防护但需谨慎维护」，有些是「文档滞后」，有些是「真实欠账」。
> 发现新问题请追加到本文件；解决一条请改状态并注明。

## A. 文档与实现矛盾（优先核实）

### #1 ff-engine lib.rs 注释与实现矛盾（已亲自核实）
- `crates/ff-engine/src/lib.rs:6-9` 写：「仍未落地：Base+Overlay 增量层、后台合并（compaction）、
  索引落盘与载入、USN 增量接线（`read_changes` 已实现但本 crate 尚无调用方）；`last_saved` 恒为 None」。
- **实证为假**：`overlay.rs:13,67` 已实现 Overlay；`inprocess.rs` 有 `poll_volume` 调 `read_changes`（约 :627/:659）、
  `save_all:959` / `load_all:964`、`last_saved` 被写入（约 :791/:874）。M2/M6 真机已验收落盘与增量。
- 可能仍成立的只有「后台合并 compaction」——源码未见 compaction 独立逻辑（Overlay 在退出/落盘时并入 Base）。
- **动作**：修正注释为准确描述；单独确认 compaction 状态。这是「文档会骗人」的头号实证。

### #2 ff-core 注释滞后
- `crates/ff-core/src/query.rs:13` 写「M7 再说：`content:` 族、`dupe:`、`runcount:`、`len:`」——
  但 `content:` 已在 M7 落地（`content.rs`）。注释滞后。
- `crates/ff-core/src/lib.rs:8`：「§8.3 用户规则 / PriorFolders 仍留接口、未实现」——这条属实（见 T2 评分规则编辑器）。
- `app/src/format.ts:6` 注释指向不存在的 `app/src-tauri/src/timeconv.rs`（实际在 `ff-engine` crate）。

## B. 明确标注的未完成（注释自述）

| # | 位置 | 内容 |
|---|---|---|
| 3 | `ff-core/src/lib.rs:8` | §8.3 用户规则 / PriorFolders 留接口未实现 |
| 4 | `ff-cli/src/main.rs:5` | `bench` / `dump` 子命令待后续里程碑（#40） |
| 5 | `ff-match/pinyin.rs:71` | 字典 `_ => return None`，未收录汉字无拼音（设计如此，覆盖有限）；多音字/双语未覆盖（#70） |
| 6 | `app/src-tauri/src/main.rs:237` | 带盘符单卷重建需引擎暴露卷列表（现忽略 volume 参数） |
| 7 | `app/src-tauri/src/main.rs:442` | `dbg_log` 注释「定位后移除」，调试日志常驻 |
| 8 | `Settings.tsx:253-345` | 11 项 pending 设置（详见 03 T2） |

## C. 生产路径 panic / unwrap 风险（需谨慎维护，非必然 bug）

- **#9 USN 解析器 slice unwrap**：`ff-ntfs/src/usn.rs:159,171-173,199-206` 解析磁盘/FSCTL 不可信字节用
  `.try_into().unwrap()`。**已核实有前置长度检查**（:152-158 短缓冲返回、:175-188 record_length 校验、
  :193 V2 头长、:208 checked_add），当前不会越界；风险在于逻辑改动破坏不变量时。
- **#10 锁中毒 panic**：ff-engine 生产路径大量 `lock().unwrap()` / `read().unwrap()`，如
  `inprocess.rs:545,630,657,791,874,916,945,951`。持锁线程 panic 会导致后续连锁 panic。
  （对比：`main.rs:451` 的 dbg_log 用 `unwrap_or_else(|e| e.into_inner())` 容忍毒化。）
- **#11 `unreachable!` 在解析路径**：`ff-core/src/query.rs:832` `_ => unreachable!("is_modifier 已过滤")`，
  依赖 `is_modifier` 前置过滤不变量；枚举变体遗漏会 panic。
- **#12 快捷键注册 `expect`**：`main.rs:699-701` 解析 `CmdOrCtrl+Alt+Space` 失败直接 panic
  （注册失败已有降级 :709-710，但解析失败没有）。

## D. 并发 / 死锁（已修复，改动锁逻辑务必知晓）

- **#13 AB-BA 死锁历史**：`inprocess.rs:620-626` 注释记录 `volumes.read()` 持锁抢 `scanner` 锁、
  `rescan_impl` 反向互抢；已三阶段分离修复，回归测试 `inprocess.rs:2792-2802` 有 8s 超时 panic 断言。
  任何改动锁顺序的打磨都要保持「任一时刻至多一把锁」并跑该测试。

## E. 死代码 / 语义不符

- **#14 `has_active_dialog` 命令已注册但前端无调用点**：`main.rs:335-338,788`；
  命令注释写「前端据此决定是否显示填入动作」，实际提示由 `sift:dialog-detected` 事件驱动（`App.tsx:258-263`）。
  要么接上、要么删。
- **#15 `categoryOf` 工具函数 UI 未用**：`app/src/data.ts:149` 注释「仅用于调试/统计（UI 暂未用）」。
- **#16 两处查询解析并存**：`app/src/engine/query.ts`（前端 MockEngine 用）与 Rust `ff-core::query` 各一套，
  概念漂移风险——改查询语法时两边都要动，或让原型只做最小子集并明确标注。

## F. unsafe 使用点（平台隔离 crate，均有不变量注释）

- `ff-ipc/src/pipe.rs:37`：`unsafe impl Send for PipeStream`（句柄单线程专有）。
- `ff-ipc/src/sid.rs:106,132`：安全描述符构建/释放（原始指针）。
- `ff-dialog/src/win.rs:75,88,135,200,218,289-303`：大量 unsafe + 手写 `#[link(name="user32")] extern`，
  未 forbid unsafe（与 ff-core/ff-match 相反）。
- `ff-ntfs/src/win32.rs:9`：每个 unsafe 块注明不变量，句柄经 VolumeHandle RAII。

## G. 资源 / 行为限制（设计取舍，非 bug）

- **#17 内容搜索硬上限**：`ff-core/content.rs:33,40,47` —— 单文件 32MiB、最多 5000 候选、总量 256MiB；
  触顶置 `ContentReport.truncated`，UI 须提示结果不完整（StatusBar 有 contentNote）。filter 串行执行（:317-320，故意）。
- **#18 各类深度上限**：`path.rs:6` parent 链 512；`search.rs:48` DEPTH_MAX 4096；
  `walk.rs:30` MAX_DEPTH 96；`import.rs:24` MAX_DEPTH 64。超限按损坏/跳过处理。
- **#19 HTTP 只有 GET /search**：无 status/rescan 端点；默认关闭（http.json），只绑 127.0.0.1、Bearer token、
  严格 Host、无 CORS、count/速率限制（安全测试见 #90）。
- **#20 网络盘不支持**：DRIVE_REMOTE 遍历无超时/取消（DESIGN §5.4）。

## H. 待接手 AI 验证的开放问题

- [ ] compaction（后台合并 Overlay→Base）究竟有没有实现？#1 修正注释前需确认。
- [ ] 重连后服务侧 EngineSession 为全新实例，旧搜索 request_id 失效（HANDOVER 已记）——是否有更好的会话恢复方案？
- [ ] 设置持久化（T1）的配置文件放 `%APPDATA%\Sift` 还是盘根 `\Sift\`？注意 index 在盘根且被 .gitignore 排除。
- [ ] filelist 导入卷如何持久化（T3）：能否复用 `<serial>.ffix` + IMPORT_SERIAL_FLAG，避免提升 FORMAT_VERSION？
- [ ] 是否有比「零新增依赖」更优先的质量项需要引入库（拼音库 ib-pinyin / 模糊 nucleo-matcher / 前端测试框架）？

---

**记录约定**：每条问题带 `file:line`；状态变化（已修/确认非问题）保留条目并注明结论与日期，不要直接删除——
这些历史能为接手 AI 省去重复踩坑。

---

## I. 深度审查新增发现（2026-11-17 · 接手 AI 复核）

> 逐文件重读源码得出，与 A–H 已知项不重复。标注「已复核」= 亲自追过触发链；其余为位置明确的待确认项。
> 行号基于提交 `6334be3`，改动后以源码为准。

### #21 [高·已复核] 索引换代后 StoredResult 持有失效引用 → range panic / open 失败
- `inprocess.rs:77-97`：结果集缓存 `Hit::Base(EntryId)` / `Hit::Overlay(frn)`，仅 `search_impl:271` 新搜索时 clear。
- 后台换代索引**不清结果集**：`poll_volume:712-714` 合并后换 `vd.index` + `vd.overlay=default()`；
  `load_volume_with:240-241` 全量重建同样换代。
- `merge_overlay_into_base:1070-1111` 按原序 push、跳过 deleted、末尾追加 overlay → 旧 eid 左移/错位或越界。
- `resolve_row:1322-1324` 用 `index.flags[*eid]`/`.size[*eid]` 直接索引 → 越界 panic；`:1343` 用
  `.get(frn).expect(...)` → overlay 清空后必 panic。open 侧 `:591-594` 有边界检查返回 NotFound，两路径行为不一致。
- 触发：出结果 → 后台合并(overlay>50000)/USN 重建/手动 rescan → 滚动取行或右键打开。
- 修法：换代 index/overlay 时按 session 淘汰 `results`，或结果集带索引代次号并在 range/open 校验。
  淘汰时勿在写锁内遍历 sessions（保持 #13「至多一把锁」）。

### #22 [高·已复核] Overlay 空名 Upsert 只回查 Base，不查自身 → 新建文件被同批清空名字
- `overlay.rs:88-97`：空名时仅 `base.frn_to_id(e.frn).and_then(base.name_str)`。
- `usn.rs:317/360`：`METADATA_CHANGE_REASONS` 且名字为空 → `Change::Upsert{ name:"" }`。
- 触发：新建文件首条 FILE_CREATE（带名）入 Overlay → 同批后续写入产生空名 Upsert → Base 查不到 →
  `oe.name` 留空 → `insert` 覆盖带名条目 → 文件从搜索结果消失。
- 这绕过 README §4「overlay 空名按 frn 从 Base 回填」关键修复的盲区（只查 Base 未查自身）。
- 修法：空名时先查 `self.added_or_changed.get(&e.frn)` 已有名字，再回退 Base。

### #23 [高·已复核] 前端从不传 max_hits，宽查询击穿虚拟滚动上限
- `App.tsx:290` search 只传 `{query,sort,opts}`，无 `max_hits`；`TauriEngine.ts:48-50` 直接透传。
- `api.rs:119` 契约注释「DESIGN 默认 10,000」，但引擎 `req.max_hits==None` 不截断（`inprocess.rs:432`），
  `total = hits.len()` 是全量命中。
- `ResultArea.tsx:122` `count=total`、`:365` 高度 `total×32px`；单卷 80 万+宽查询多卷可达百万级 →
  滚动高度超 Chromium 上限（~3355 万 px），虚拟化定位失效；`rows` 随滚动只增不减（`:174`）内存膨胀。
- 修法：`search({...,max_hits:10000})`，触顶时在状态栏提示结果已截断；对窗口外 rows 做淘汰。

### #24 [中·已复核] 虚拟滚动 range 回包无 .catch
- `ResultArea.tsx:172-181` `engine.range(...).then(...)` 无 catch；`App.tsx:298` 首屏 range 有 `.catch(()=>[])`。
- TTL 60s 回收 / id 失效后滚动 → rejected Promise 未处理 + 缺失行永久空白。修法：补 catch 并标记 id 失效。

### #25 [中·已复核] 事件监听在 StrictMode 清理竞态下泄漏
- `main.tsx:10` 启用 StrictMode；`App.tsx:246-266` 五个 `listen().then(u => unlistens.push(u))` 在 Promise
  兑现后才登记，cleanup 若先于兑现执行则数组为空 → 监听永不注销 → toast/动画重复触发。
- 修法：加 `cancelled` 标志，`.then` 内若已取消则立即 `u()`。

### #26 [中·已复核] ParseError.span 字节偏移被当成字符位展示
- Rust `ParseError.span` 为**字节**区间（`lib.rs:91-93`、`types.ts:31-32` 注释明确）；
  `i18n.ts:21` 用 `pos+1` 直接当「第 N 位」，`TopBar.tsx:176-178` 直接传入；Mock 侧 `query.ts` 用 UTF-16 下标。
- 错误位前有中文时真机/Mock 显示位置不一致且均非字符位。修法：前端按 `Array.from(query.slice(0,start)).length` 换算。

### #27 [中·已复核] 搜索失败不清旧结果，状态栏对错误查询显示旧命中
- `App.tsx:315-321` catch 仅收 spinner，不重置 summary/rows/total/parseError；
  `StatusBar.tsx:38-39` 仍按新 query 显示旧 `summary.total`/`elapsed_ms`（UI 撒谎）。修法：与空查询分支一致清空。

### #28 [中·已复核] 卷枚举第三遍未过滤 drive_type，CDROM/RAM 盘重新混入
- `win32.rs:118-136` 第一遍按 serial 收全部 GUID 卷（无 drive_type 过滤）；第二遍 `:157` 排除光驱/网络/RAM；
  第三遍 `:178-191` 补无盘符卷只查 `has_mount_points`，未再过滤 drive_type → 被第二遍排除的卷又加回（drive:None）。
- 后果：本应排除的卷进入扫描，`walk.rs` logical_root 返回 Unsupported 或触发无谓 MFT 全扫。修法：第三遍套用同一过滤。

### #29 [中低·已复核] USN V2 FileNameOffset 未校验下界
- `usn.rs:208-217` 仅校验 `name_offset+name_length<=record_length`，未校验 `name_offset>=USN_V2_HEADER_LEN(60)`。
- 损坏记录可令偏移指向固定头内部，解码出垃圾文件名且 `stopped_at` 仍为 None。修法：追加下界与 `name_length%2==0` 校验。

### 低危（位置明确，待确认影响）
| # | 位置 | 问题 |
|---|---|---|
| 30 | `win32.rs:133-135` | `FindNextVolumeW` 任何错误都 break，非 NO_MORE_FILES 也静默吞掉 |
| 31 | `usn.rs:320` vs `:359` | `UsnEvent::Upsert.parent_frn` 未掩码，与 `Change` 口径不一（ff-core 统一 mask 故暂无实害） |
| 32 | `ResultArea.tsx:170-182` | range 请求无在途去重，慢滚动可发重叠请求 |
| 33 | `main.rs:393-401` | toggle_window 无法取消进行中隐藏，90ms 内连按只隐藏不唤回 |
| 34 | `App.tsx:127-128` | toastTimer/spinnerTimer 无卸载清理（仅 searchTimer 有） |
| 35 | `App.tsx:515` | `.search-view` 的 onAnimationEnd 未校验 target，子动画冒泡会提前结束进入动画 |
| 36 | `actions.rs:34`/`inprocess.rs:601` | CopyName 两后端取名方式不一致；Pipe 侧多一层空名校验 |

## J. 打磨优先级建议（用户可感知价值 × 成本）
1. **#23 max_hits**（一行改动，直接解决宽查询 UI 卡死/空白）→ 2. **#24 range catch**（一行，消除永久空白行）。
2. **#21/#22 两个 overlay/结果集一致性 bug**（真机增量场景才会触发，建议各补一条回归测试再改）。
3. **#25/#27/#26 前端健壮性**（StrictMode 泄漏、失败清态、错误位置换算，成本低）。
4. **#28/#29 ntfs 健壮性** → **低危表批量清理**。
- 三项清理类旧账（T7：lib.rs 过期注释、dbg_log 常驻、has_active_dialog 死代码）仍有效，可随手带走。

## K. 索引复用缺陷（2026-11-17 发现并修复）

### #37 [高·已修复] 启动无条件全量重扫，磁盘索引缓存形同虚设
- 现象：DESIGN §6 承诺「启动载入已有索引 ≤0.5s」，但两个启动路径 `load_all` 之后紧接着
  对所有卷 `mark_pending` + `rescan`（`full_scan` 全量 MFT 扫描），把刚载入的索引整个覆盖。
  每次开程序都重新索引，磁盘持久化毫无收益——正是用户观察到的效率问题。
- 位置：`app/src-tauri/src/main.rs:544-551`、`crates/ff-service/src/main.rs:304-309`（旧行号）。
- 根因之一（#38）：落盘的续读起点恒为 0，即便不重扫也无法增量续读（见下条）。
- 修法：引擎新增 `startup_reconcile(&[VolumeId])`，只挑出「新卷 / 空索引 / 失效卷」做全扫；
  已载入卷保持 Ready，由后台 `poll_volume` 从 `next_usn` 续读关机期间的变更。
  `load_all` 同时用 `supports_incremental` 修正非 NTFS 卷的增量标志（否则误判有 USN 会永不重建）。
- 验证：`startup_reconcile_only_returns_missing_or_empty_volumes` 单测。

### #38 [高·已修复] 落盘索引的 USN 续读起点恒为 0（Bug #6 的落盘盲区）
- `load_volume_with:233-239` 把 `ScanMeta.journal_id/next_usn` 只写进运行时 `vd.info`；
  但 `save_volume:790` 落盘用 `vd.index`，其 `volume` 是 `IndexBuilder::new(Default)` 的快照
  （journal_id=0/next_usn=0）。Bug #6 那次修复只覆盖运行时，未覆盖落盘路径。
- 后果：重启后从 `next_usn=0` 续读 → `read_changes` 判 `from_usn < first_usn` → `JournalUnavailable`
  → `rebuild_volume` 全量重建。磁盘缓存即使载入了也必然被弃用，形成「每次启动全量重扫」闭环。
- 修法：
  - `ff-core/persist.rs` 新增 `VolumeIndex::save_with_volume(path, &VolumeInfo)`，落盘时用运行时权威
    `vd.info` 覆盖 `index.volume` 的元数据（journal 起点随增量推进），`save()` 保持原语义委派给它。
  - `save_volume` 改用 `save_with_volume(&path, &vd.info)`。
  - `load_volume_with` 在赋值 `vd.index` 前把 `vd.info` 同步进 `index.volume`，让任何直接
    `index.save()` 的路径也不会写回 0。
- 自愈链确认：`read_changes` 对 journal_id 不匹配 / next_usn 落后均返回 `JournalUnavailable`，
  `poll_volume` 自动 `rebuild_volume`，故旧版本留下的 0 起点索引重启后也能自愈重建一次。
- 验证：`save_persists_runtime_usn_position_not_index_snapshot` 单测（推进后落盘再载入，断言起点保留）。

### 修复影响面与验证
- 改动文件：`ff-core/src/persist.rs`、`ff-engine/src/inprocess.rs`、`ff-service/src/main.rs`、`app/src-tauri/src/main.rs`。
- 锁序：`load_all` 改为三阶段（读盘 → 锁外查增量通道 → 一次性写 vols），避免在 volumes 写锁下做 IO / 与 scanner 锁形成 AB-BA。
- 门禁：`cargo test --workspace` 全绿、`cargo clippy --workspace --all-targets` 无告警、两个独立 crate 均 `cargo check` 通过。
- 未做：真机重启计时（旧索引首次自愈重建一次，之后应秒开）——需管理员 + 真实 NTFS 卷，不进 CI。

## L. 第二轮审查发现与修复（2026-11-17）

### #39 [中·已修复] HTTP Bearer 校验在非 ASCII 头值上 panic
- `ff-http/src/security.rs:46` 用 `value[..7]` 按字节切片 UTF-8 String，第 7 字节落在多字节字符中间时
  panic（char boundary）。头值由远端控制（`server.rs:207` trim 后入 HashMap），pure function 契约被破坏。
- 修法：全程 `as_bytes()` 比较，`bytes[..7].eq_ignore_ascii_case(b"Bearer ")`，candidate 也用字节切片。
- 验证：`bearer_non_ascii_prefix_does_not_panic` 单测。

### #40 [中·已修复] 反序列化索引 name_off 越界可 panic
- `ff-core/src/entry.rs:332` `&arena[start..end.max(start)]` 在 name_off 越界时 panic；
  `persist.rs:load` 只校验各列长度相等 + CRC（CRC 算法公开可重算），不校验 name_off 语义。
- 修法：`persist.rs` 新增 `name_off_valid()`，load 在 CRC 后校验首项 0/单调不减/每项 <= names.len()，
  非法返回 None → 触发全量重建；`entry.rs:name_in` 改用 `arena.get(start..hi).unwrap_or(&[])` 消除残余 panic。
- 验证：`name_off_valid_rejects_malformed_offsets` 单测。

### #41 [中·已修复] 对话框钩子安装失败被静默吞掉
- `ff-dialog/src/win.rs:145` hook_thread 无条件先 `id_tx.send(GetCurrentThreadId())` 再判 `hook.is_null()`，
  导致 `start():177` 的 `thread_id==0` 失败分支永不可达；SetWinEventHook 失败时功能静默失效。
- 修法：`Sender<Option<u32>>` 回传安装结果，失败发 None，`start()` 用 `ok_or_else` 返回 Err。
- 验证：编译通过（真机钩子失败路径难构造，逻辑开门见山）。

### #42 [低·已修复] 管道干净断开未转 EOF（注释与实现矛盾）
- `ff-ipc/src/pipe.rs:100-105` 注释称 ERROR_BROKEN_PIPE 转 EOF(Ok(0))，代码却 `map_err(io::Error::other)` 直接传播，
  正常关闭被当连接错误、服务侧误记日志，read_message 的干净断开/截断帧区分失效。
- 修法：ReadFile 失败取 HRESULT 低 16 位（同 ff-ntfs::win32_code），109/232 返回 Ok(0)，其余 Err。

### #44 [低·确认不修] 未闭合分组的 case:/content: 副作用残留
- `query.rs:parse_group` 未闭合回退时 `pos=open_pos+1`，外层 parse_and 会把 `case:foo` 重新解析一次，
  修饰符副作用重现；单在 parse_group 回滚挡不住。且 case: 是全局修饰符，用户打了即生效语义上可接受。
- 尝试修法引入测试失败（回滚后外层重解析重现副作用），已撤销，保持原行为。属打字中途容错边界，不强行改。

### 真机验证（本轮）
- 管理员 + C/D/F 三 NTFS 卷；`cargo test -p sift-tests -- --ignored` 4/4 通过。
- `ff-cli scan C:` 全量 854,721 条目 / 22.557s，峰值 WS 148.86 MiB，扫描前 NextUSN=27076950472（非0，印证 #38）。
- debug exe 双击白屏根因：debug build 走 `devUrl=localhost:5173`，依赖 vite dev server。
- **[更正 2026-11-17] release 双击白屏（"localhost 拒绝连接"）根因**：手动 `cargo build --release`
  不带 `--features custom-protocol` 时，Tauri 的 `cfg(dev)` 仍为真 → 走 `devUrl=localhost:5173`
  → 无 dev server 时报 ERR_CONNECTION_REFUSED。之前"release 用 frontendDist 可独立打开"的说法
  **只在 `tauri build`（自动带该 feature）或显式 `--features custom-protocol` 时成立**。
  `app/src-tauri/Cargo.toml` features 段已注释此坑。正确命令：
  `cd app/src-tauri && cargo build --release --features custom-protocol`（或 `npm run build:tauri`）。
  2026-11-17 17:41 用正确命令重建后截图验证：设置面板正常渲染，非错误页。
- 门禁：`cargo test --workspace` 全绿、`cargo clippy --workspace --all-targets` 无告警。

## M. 索引文件生命周期专项（2026-11-17）

用户问「索引是否堆积、直接放程序根目录是否正常」。核实：`index_dir()` 取 exe 所在**盘符根**拼
`<盘符>:\Sift`（main.rs:566），本例程序恰在 `D:\Sift` 故与程序目录重合，并非程序写死在 exe 旁。
磁盘实况：3 个 `.ffix` 共 106 MB（对应 C/D/F 三卷，按 serial 命名，属正常量）。

### 已修（F1/F4/F7，本轮）
- **F1 服务模式不清理**：`prune_stale_indexes` 原只在桌面 `build_engine()` 内，`build_backend()` 选
  PipeBackend 时不执行；ff-service 的 `spawn_initial_scan` 从无清理。换盘/拔盘/serial 变化的旧
  `.ffix` 在两处各自堆积。修：清理逻辑下沉为 `ff-engine::prune_index_dir(dir, &valid_serials)`，
  桌面与服务共用；服务 `spawn_initial_scan` 枚举成功后调用。
- **F4 空集删光**：原守卫只挡 `enumerate_volumes() == Err`，`Ok(vec![])` 时 valid_serials 为空，
  prune 会把全部合法 `.ffix` 判 stale 删光。修：两处 match 加 `Ok(vols) if !vols.is_empty()` guard，
  空集与 Err 一样跳过清理。
- **F7 tmp 残留**：落盘是原子的（写 `<serial>.tmp` → rename），崩溃在中间残留的 `.tmp` 无人清理，
  且三处清理只认 `ffix`。修：`prune_index_dir` 同时删除 `*.tmp`。
- 新模块 `crates/ff-engine/src/persist_cleanup.rs`，3 条单测（stale+tmp 删除 / 空集不删 /
  不可解析文件名保留）；workspace 测试绿、clippy 无告警。
- 顺带修正 `ff-engine/src/lib.rs` 顶部过期注释（长期 T7 欠账）：原写「落盘/USN 未落地、
  last_saved 恒 None」，与现状矛盾。

### 记录但不修（风险/收益权衡）
- **F2 双索引目录**：桌面 InProcess 写 `<盘符>:\Sift`，服务写 `%ProgramData%\Sift`；切换后端
  会各存一份且互不清理。统一需处理后端切换时的索引迁移 + ACL，牵涉较广，单独评估。
- **F5 退出写放大**：`flush()->save_all()` 无条件重写全部 Ready 卷（退出即重写 106 MB），
  `maybe_save()` 用全局 dirty 任一卷变脏就全量重写。改为 per-volume dirty 有「漏置位→变更
  永不落盘」的数据丢失风险，收益仅是减少 SSD 写入，暂不动核心落盘逻辑。
- **F6 克隆盘同 serial**：磁盘克隆会复制卷序列号，两卷同 serial 共用 `<serial>.ffix` 互相覆盖；
  载入用文件内 drive 重建键，盘符漂移可能串卷。属罕见场景，需复合键（serial+GUID）+ 迁移。
- **F8 离线卷误删**：`valid_serials` 来自 `enumerate_volumes`，排除网络盘/光驱/RAM 盘/离线卷；
  曾落盘的这类卷再启动会被判 stale 删除，卷恢复后需重扫。可考虑延迟删除。
