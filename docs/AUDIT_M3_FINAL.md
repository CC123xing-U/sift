# Sift M3 全面代码正确性审查 —— 最终报告

> 日期：2026-10-26 · 审查对象：git 工作树（main @ `4b65e04` + 未提交改动）
> 两轮工作：10-25 全量审计与修复 / 10-26 复查 + GUI 实机全流程实测
> 门禁终态：`cargo fmt --check` 绿 · `cargo clippy -D warnings` 双 workspace 绿 · `cargo test --workspace` 179 绿 · release 构建链接成功 · **GUI 实机启动-索引-落盘全流程走通**

---

## 0. 结论摘要

| # | 问题 | 级别 | 状态 |
|---|---|---|---|
| 1 | D: 卷扫描永不完成（"卡死"）= `scanner` Mutex 与 `volumes` RwLock 的 **AB-BA 死锁** | P0 | **已修复 + 回归测试** |
| 2 | 进程无 panic 无 WER 死亡 = 死锁冻结（主因）+ C: 盘满导致 WebView2 子进程故障（独立环境因素） | P0 | 主因已修复；环境因素给出验证方法 |
| 3 | 「索引出错：未知」= 错误信息链路断裂 | P1 | **已修复** |
| 4 | 进度恒 0% = `progress` 从未赋值 | P1 | **已修复**（100ms 节流原子量） |
| 5 | 持久化未接线 = `set_save_dir` 从未被调用 | P2 | **已修复**（实机已验证落盘 91.4MB C: 索引） |
| 6 | **索引完成后无限循环重建**（`ScanMeta` 被丢弃 → `from_usn=0` → `JournalUnavailable` → rebuild 死循环） | P0（新发现） | **已定位，按指示暂不修** |
| 7 | `maybe_save` 不检查卷状态 → 空索引覆盖好索引 | P1（新发现） | 已定位，暂不修 |
| 8 | `load_all` 复活已卸载的幽灵卷 | P2（新发现） | 已定位，暂不修 |
| 9 | 落盘目录在 C:（盘满时持久化静默失效 + 吃掉最后空间） | P2（新发现） | 已定位，暂不修 |

**当前程序可以正常打开与运行**：窗口显示、托盘建立、焦点守卫生效、3 个卷全部扫描完成、索引成功落盘、进程全程存活（10-26 23:03 实测，见 §4）。残留问题是 #6 的循环重建（浪费 CPU/磁盘、UI 反复显示"正在建立索引"）与 #7–#9，均已有根因与修复方案。

---

## 1. 第一轮（10-25）：全量审计与 P0/P1/P2 修复

### 1.1 审计范围

逐文件读完：`docs/{DESIGN,DECISIONS,BENCH,UI_SPEC}.md`；`crates/` 全部 21 个 .rs（ff-core 8 模块 / ff-match 2 / ff-ntfs 3 / ff-engine 3 / ff-cli 1 + bench/tests）；`app/src-tauri/`（main.rs、build.rs、两个 Cargo.toml、tauri.conf.json）；`app/src/` 全部 15 个 ts/tsx + styles.css + index.html + package.json + vite/tsconfig。对照 DESIGN 逐节核对合规性（结论：已实现功能全部合规；未实现项均属 M4–M7 计划内推迟，DECISIONS 有记录）。

### 1.2 隔离实验（排除扫描器嫌疑）

`ff-cli`（无 Tauri/WebView2）走完整引擎路径（`InProcessEngine::new()` + `rescan()` + `search()`）：

| 卷 | 条目数 | scan 耗时 | search 路径耗时 |
|---|---:|---:|---:|
| D: | 273,554 | 2.832 s | 2.1 s |
| C: | 900,252 | 4.524 s | 4.7 s |
| F: | 688,387 | 4.014 s | 3.3 s |

**扫描器与 ntfs-reader 无死循环、无病理慢** → 卡死只出现在 Tauri app 的多卷顺序扫描路径。

### 1.3 P0 根因：AB-BA 死锁（证据链）

锁序反转（修复前行号）：

| 线程 | 顺序 | 证据 |
|---|---|---|
| 扫描线程 | `scanner` Mutex → `volumes.write()` | `inprocess.rs:344`（rescan_impl 持锁跑全扫描）→ `inprocess.rs:160/171`（load_volume_with 收尾写锁） |
| 后台线程 | `volumes.read()` → `scanner` Mutex | `inprocess.rs:402` → `inprocess.rs:412`（poll_volume 持读锁抢 scanner） |

触发序列：C: 扫完变 Ready → D: 开扫（持 scanner 数秒）→ 后台 1s 周期轮询 C:（持 `volumes.read()` 等 scanner）→ D: 扫完要 `volumes.write()`（被读锁挡）→ **环路闭合，双线程互等**。

- **为何 ff-cli 不死锁**：单卷扫描期间 map 中无 Ready 卷，`poll_volume` 在状态检查处 early return，永不触及 scanner 锁。
- **为何 Tauri 必死锁**：`main.rs` 后台线程顺序扫 5 卷，第一卷 Ready 后条件即成立。
- **复现测试**（`inprocess.rs` 新增 `background_poll_does_not_deadlock_with_rescan`，FakeScanner 慢扫 2.5s + 预置 Ready 卷 A）：修复前 **8.34s panic「死锁」**；修复后 **2.90s 通过**。

### 1.4 已实施修复（第一轮）

1. **P0**：`poll_volume` 重排为三阶段（快照释放读锁 → 独占 scanner 做 IO → 写锁应用），任何时刻后台线程至多持一把锁。
2. **P1 错误链路**：`VolumeData` 增 `message: Option<String>`；`load_volume_with` 错误分支存 `e.to_string()`；`status_impl` 读 `vd.message.clone()`（原硬编码 `None`）。
3. **P1 进度**：`VolumeData` 增 `scan_progress: Arc<AtomicU64>`（千分比）；sink 闭包 100ms 节流更新；`status_impl` 在 Scanning 态换算 `Some(p/1000)`。分母取上一轮 entry_count。
4. **P2 持久化**：`main.rs::build_engine` 启动时 `set_save_dir(%LOCALAPPDATA%\Sift)` + `load_all()`；`let _ = rescan(...)` 改为记录错误。

### 1.5 进程死亡的确定性解释

死锁只造成**冻结**（UI 线程的 `status()` 只需读锁，仍响应 → 表现为"卡死但窗口还在"）。**死亡是独立事件**：C: 0 GB → WebView2 用户数据目录（默认 `%LOCALAPPDATA%`，在 C:）写缓存失败 → WebView2 浏览器子进程故障 → wry 销毁 webview → tao 事件循环正常退出 → `exit(0)`：无 Rust panic（stderr 空）、无 sift-app.exe 的 WER 记录（崩的是 WebView2 子进程）。验证方法：`set WEBVIEW2_USER_DATA_FOLDER=D:\SiftWebView` 或释放 C: 空间后复测。

---

## 2. 第二轮（10-26）：复查 + GUI 实机全流程

### 2.1 工作树复查

- 第一轮全部修复在位（`inprocess.rs` 三阶段 poll_volume / message / scan_progress；`main.rs` 持久化接线）。
- 会话间新增改动（他人/他次会话）已复核，均合理：
  - `ff-ntfs/win32.rs` 增 `has_mount_points()`：`enumerate_volumes` 跳过无挂载点卷（5 卷 → 3 卷），解释了原"2 个无盘符卷 1–2ms 完成"——它们是未挂载/特殊卷，现直接不枚举；
  - 前端 `overallScanProgress()`：多卷进度取均值替代 max；
  - `tauri.conf.json`：build 命令去 `--prefix ..`、去 `theme: Auto`。
- `app/dist` 与源码时间戳核对一致（重建产物哈希不变）。

### 2.2 门禁复跑

`cargo test --workspace` **179 passed / 0 failed**（ff-engine 20/20 含死锁回归）；`cargo clippy -D warnings` 根 workspace 与 `app/src-tauri` 双绿（修了一处 `doc_lazy_continuation`）；`cargo fmt --check` 绿；`cargo build --release --features custom-protocol` 成功（1m32s；期间一次"卡住"实为 cargo 文件锁等待 + C: 盘满导致 crate 下载/解压缓慢，非代码问题）。

### 2.3 GUI 实机全流程（PID 17656，23:03:25 启动，`%TEMP%\sift-debug.log` 实测）

```
23:03:25.785  engine: loaded persisted indexes from "C:\Users\Administrator\AppData\Local\Sift"   ← P2 生效
23:03:25.786  engine: 3 volume(s) found                                                          ← 挂载点过滤生效
23:03:27.581  setup: begin / 23:03:27.606 setup: done (tray built, engine managed)                ← 托盘 OK
23:03:27.617  window focused event: true (self_pid=17656, fg_pid=17656)                           ← 窗口显示并获焦
23:03:31.222  engine: rescan done   (C: 5.4s)
23:03:40.013  engine: rescan done   (D: 8.8s)   ← 修复前 D: 永不完成
23:03:49.361  engine: rescan done   (F: 9.3s)
23:03:38      %LOCALAPPDATA%\Sift\2661335377.ffix 落盘 91,419,352 B（C: 全量索引）                ← 持久化实机验证
```

进程全程存活、无 panic、无 WER；10-25 15:06 的一次运行（PID 23344）同样全序列干净。**D: 卷 `rescan done` 在真实 GUI 中出现**——P0 修复的实机确认。

### 2.4 落盘目录取证（`%LOCALAPPDATA%\Sift\`）

| 文件 | 大小 | mtime | 解读 |
|---|---:|---|---|
| `2661335377.ffix` (C:) | 91.4 MB | 10-26 23:03:38 | 本轮成功落盘 |
| `549943042.ffix` (D:) | 24.9 MB | 10-25 15:01:39 | **D: 扫描完成的历史铁证**（修复前不可能存在） |
| `2661335377.ffix` 旧值 | 120 B | 10-25 14:10:05 | **Bug #7 证据**：启动 1s 时 C: 尚处 Scanning（空索引）即被 `maybe_save` 落盘覆盖 |
| `*.tmp`（0 B ×4） | 0 | 多次 | C: 盘满时 `fs::write` 失败的残留（原子写的 tmp 阶段） |
| `2281921499/816230857.ffix` | 120 B | 10-26 23:04:53 | **Bug #8 证据**：本轮只枚举 3 卷，这两个已卸载幽灵卷却被 `load_all` 复活并重存 |

---

## 3. 新发现 bug（已定位，按指示暂不修）

### Bug #6（P0）：索引完成后无限循环重建

**证据链（代码级，确定）**：

1. `inprocess.rs:182` `Ok(_meta) =>` —— `full_scan` 返回的 `ScanMeta`（含 `journal_id`/`next_usn`，`scanner.rs:134-140`）被**丢弃**；
2. `inprocess.rs:189` `vd.info = info`，而 `info` 是 `VolumeInfo { serial, drive, ..Default::default() }` → **`journal_id=0`、`next_usn=0`**；
3. 后台 `poll_volume` 用 `from_usn=0` 调 `read_changes` → `scanner.rs:165` `if from_usn < info.first_usn` 恒真（任何有历史的卷 first_usn>0）→ 返回 `JournalUnavailable`；
4. `inprocess.rs:486` `JournalUnavailable → rebuild_volume` → 全量重扫 → 回到第 2 步 → **每卷每隔「扫描耗时+1s」无限重建**。

**旁证**：DESIGN §5.2 明确要求"扫描前记录 next_usn，扫描期间的变化由续读补齐"——当前实现记录后丢弃，M2 的 USN 增量在真实运行中**从未生效**（每次"增量轮询"都退化为全量重建）。UI 表现为索引完成后仍反复进入"正在建立索引"。dbg_log 不可见是因为 `rebuild_volume` 路径无日志（与 `build_engine` 首轮扫描的日志不同源）。

**修复方案（5 行，待实施）**：`load_volume_with` 成功分支改为

```rust
Ok(meta) => {
    vd.info = VolumeInfo {
        serial: vol.serial,
        drive: vol.drive,
        guid_path: vd.info.guid_path.clone(),   // 保留
        journal_id: meta.journal_id,            // ← 接住 ScanMeta
        next_usn: meta.next_usn,                // ← 接住 ScanMeta
    };
    ...
}
```

**验证方法**：新增 FakeScanner 单测——`read_changes` 在 `from_usn < 首条 usn` 时返回 `JournalUnavailable`（模拟真实 Win32 语义），断言 `rescan` 后 `poll_once()` 不触发第二次 `full_scan`（用调用计数断言 =1）；实机观察 dbg_log/CPU 不再出现周期性重扫。

### Bug #7（P1）：`maybe_save` 不检查卷状态，空索引覆盖好索引

`inprocess.rs:504-516`：遍历全部卷直接 `save_volume`，Scanning/Error 态的空白索引（120 B）会覆盖磁盘上的好索引。证据：`2661335377.ffix` 曾为 120 B（10-25 14:10:05，启动后 1s，C: 尚在 Scanning）。若进程在重扫完成前死亡，下次启动将载入空索引。
**修复**：`maybe_save`/`save_all` 跳过 `state != Ready` 的卷（`save_volume` 内加一行判断）。

### Bug #8（P2）：`load_all` 复活幽灵卷

`load_all` 无条件载入目录下所有 `.ffix`，不与 `enumerate_volumes()` 结果对齐 → 已卸载/过滤掉的卷以 Ready+0 条目形态驻留 UI 并被反复重存（证据：23:04:53 两个 120 B 幽灵 ffix 被重写）。
**修复**：`build_engine` 中 `load_all` 后按当次枚举的 serial 集合 prune；或 `load_all` 接受允许集合参数。

### Bug #9（P2）：落盘目录位于 C:，盘满时持久化静默失效

DESIGN §6 规定开发期 `%LOCALAPPDATA%\Sift`（C:）。实测 C: 仅剩 88 MB 时：91.4 MB 的 C: 索引落盘**吃掉最后空间**，其后所有 save 失败（0 B `.tmp` 残留），且 WebView2 缓存同受牵连（关联第一轮的进程死亡假设）。`save_volume` 的错误被 `let _ =` 吞掉，无日志。
**修复建议**：save 失败写入 `VolumeData.message`（或 tracing）使 UI/日志可见；并在 C: 空间不足时给出提示或允许配置落盘目录（属 DESIGN §6 变更，需你确认）。

---

## 4. 验证证据汇总

| 项 | 命令 | 结果 |
|---|---|---|
| 死锁复现（修复前） | `cargo test -p ff-engine background_poll_does_not_deadlock` | 8.34s panic「死锁」 |
| 死锁修复（修复后） | 同上 | 2.90s ok |
| 引擎单测 | `cargo test -p ff-engine` | 20/20 绿 |
| 全 workspace | `cargo test --workspace` | 179 passed / 0 failed |
| Lint | `cargo clippy --workspace --all-targets -- -D warnings` + app/src-tauri 同 | 双绿 |
| 格式 | `cargo fmt --all -- --check` | 绿 |
| 隔离实验 | `ff-cli scan/search C:/D:/F:` | 2.8–4.7 s/卷，无卡死 |
| GUI 实机 | 直接运行 release exe + dbg_log + 落盘目录取证 | 启动→托盘→焦点→3 卷 rescan done→91.4MB 落盘，全程存活 |

## 5. 量化回答

- **248k 文件全量扫描预期耗时**：独立路径 2.1–2.8 s（ff-cli 实测）；app 内 8.8 s（含 UI 轮询/内存压力的实测上限）。Bug #6 修复后不再重复扫描，稳态为每启动一次。
- **`cargo test -p ff-engine`**：20/20 全绿（19 原有 + 1 死锁回归）。
- **DESIGN §13-M3「按键→首屏 ≤50ms」**：无负面影响。search 路径只持 `volumes.read()`（与修复前一致），M1 基准子串 2.5 ms @2M 条，273k 条约 1 ms + IPC 1–3 ms ⇒ ≤10 ms。持久化接线后启动即载入旧索引可搜，首屏不再等扫描。

## 6. 修改文件清单（相对 `4b65e04`）

| 文件 | 内容 |
|---|---|
| `crates/ff-engine/src/inprocess.rs` | P0 三阶段 poll_volume；P1 message/scan_progress；VolumeData/blank_volume/add_volume/load_all 适配；死锁回归测试 |
| `app/src-tauri/src/main.rs` | P2 set_save_dir+load_all；rescan 错误记录 |
| （会话间他人改动，已复核） | `ff-ntfs/win32.rs` has_mount_points；前端 overallScanProgress；tauri.conf.json 两处 |

Bug #6–#9 的修复**未实施**（按指示先出报告）。建议实施顺序：#6 → #7 → #8 → #9。

---

## 7. 处置状态更新（2026-10-26 23:25，总指挥）

| Bug | 状态 | 依据 |
|---|---|---|
| **#6 无限循环重建（P0）** | **已修复 + 回归测试 + 实机验证** | `inprocess.rs::load_volume_with` 成功分支接住 `ScanMeta`（`journal_id`/`next_usn`），`guid_path` 从旧 info 继承；新增 `scan_meta_is_kept_so_poll_does_not_rebuild_forever`（含**负向对照**：退回 `next_usn=0` 后测试第 1 轮即失败）；实机 release 运行 30s 墙钟仅耗 2.03s CPU（6.8%），日志无重复重建 |
| **#7 空索引覆盖好索引（P1）** | **已修复** | `save_volume` 增 `state != Ready` 短路 |
| **#8 幽灵卷（P2）** | 未实施 | 已写入 DECISIONS #65，待你确认 |
| **#9 落盘目录在 C:（P2）** | 未实施 | 属 DESIGN §6 变更，需你确认 |

### 门禁终态（本次修复后重跑）

| 门禁 | 结果 |
|---|---|
| `cargo fmt --all -- --check`（双 workspace） | 绿 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 绿 |
| `cargo test --workspace` | **180 passed / 0 failed**（ff-engine 21 含新增回归） |
| `cargo build --release --features custom-protocol` | 绿，`sift-app.exe` 9.59 MB |
| 实机运行 | 3 卷各一次 `rescan done` → 稳态 CPU 6.8% → 进程稳定存活 |

### 顺带更正（DECISIONS #63 的技术细节）

`cfg(dev)` **不是**按 debug/release profile 判定，而是由 tauri 2.12.5 build.rs 依 `tauri` 依赖是否启用 **`custom-protocol`** feature 决定。故分发/测试务必用 `cargo build --release --features custom-protocol` 或 `npm run build:tauri`；裸 `cargo build --release` 仍会走 devUrl（localhost 白屏）。已写入 DECISIONS #63 更正段。

### 新增观察（非阻塞，建议后续）

`SAVE_INTERVAL = 10s` 且无脏标记 → 稳态每 10 秒重写全部索引（实测 3 卷 ~177 MB/次）。功能正确，I/O 浪费，建议加 dirty 标记或拉长间隔。已并入 DECISIONS #65。
