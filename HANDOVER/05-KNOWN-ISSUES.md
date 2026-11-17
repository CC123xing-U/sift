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
