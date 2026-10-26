# Sift M3 代码全面审查任务书（转交审查 AI 用）

> 使用方法：把本文件全文（或下方 prompt 正文）交给另一个具备本仓库（D:\Sift）文件访问权限的 AI。
> 它应直接读仓库代码与 docs，不信任何二手描述。

---

## Prompt 正文（从这里开始复制）

你是资深 Rust + Windows 系统级开发工程师，擅长性能剖析与并发缺陷定位。请对 **Sift**（Windows 本地文件搜索工具，Rust workspace + Tauri v2 桌面外壳）做一次**全面代码正确性审查**。当前里程碑 M3（Tauri 外壳 + 进程内引擎）刚落地，实机测试发现引擎侧疑似缺陷，**用户报告"卡死"**。你的任务：定位根因（带证据链）、按优先级给出修复方案、说明验证方法。

### 一、必读文档（按顺序，先读再动手）
1. `D:\Sift\docs\DESIGN.md` — 总体设计。重点：§5 索引与 USN journal、§9 引擎接口契约（EngineClient）、§11 UI 行为、§13 里程碑验收标准。
2. `D:\Sift\docs\DECISIONS.md` — 全部架构决策。#58–#63 与当前问题直接相关（#63 记录了 debug/release 走 devUrl 的根因）。
3. `D:\Sift\docs\BENCH.md` — 性能基线与已验证项。
4. `D:\Sift\docs\UI_SPEC.md` — 前端规范（了解状态栏/进度 UI 契约）。

**注意：审查对象是工作区当前状态（git 工作树有未提交改动，见下），不是某个 commit。所有结论必须带 `文件:行号` 证据，禁止猜测式结论。**

### 二、代码地图
- `crates/ff-core/` — 零平台依赖核心：`VolumeIndex`（SoA 列存）、`IndexBuilder`、查询解析、持久化 FFIX（persist.rs）。
- `crates/ff-ntfs/` — Windows NTFS 扫描器（scanner.rs 等），底层用 `ntfs-reader` crate。
- `crates/ff-engine/` — `InProcessEngine`（inprocess.rs：全量重建/增量轮询/Overlay/后台循环/落盘）+ `api.rs`（EngineClient 契约与 IPC 类型）。
- `app/src-tauri/` — Tauri v2 外壳（main.rs：命令注册、托盘、全局热键、焦点自动隐藏、**临时的 dbg_log 调试日志**）。
- `app/src/` — React 前端（TauriEngine.ts 走 IPC；MockEngine.ts 是测试替身）。

### 三、实机症状与证据（全部为实测，非转述）
环境：Windows 11 · 提权 Administrator · **C: 盘 0 GB 可用（满）** · D: 为工作盘（约 24.8 万个文件）。
运行方式：`cargo build --release --features custom-protocol` → 直接运行 `app/src-tauri/target/release/sift-app.exe`。

1. **【P0 核心】D: 卷扫描永不完成（卡死）**：UI 停在「正在建立索引… 0%」。引擎日志（`main.rs` 的 `dbg_log` 写 `%TEMP%\sift-debug.log`）实测时间线：
   - `enumerate_volumes()` 发现 **5 个卷**；其中 2 个无盘符卷（serial 2281921499 / 816230857）**1–2ms 就"完成"**（可疑：过快，疑似吞了错误或扫了空卷）；
   - C: 卷全量扫描约 **4.5s 完成**（正常）；
   - **D: 卷扫描启动后，任何一次运行都没出现过 `rescan done`**（观察超过 40s；D: 仅 24.8 万文件，秒级即可完成 → 病理级慢或死循环）。
2. **【P0】进程在 D: 扫描途中死亡**：一次监控中 t=23s 退出；另一次 t=40s 后、3 分钟内死亡。退出时：Windows 事件日志**无 WER 崩溃记录**（只有 C 盘满导致的 Windows Search 损坏告警）；stderr 重定向到文件捕获为**空**（即**没有 Rust panic 消息**）。请给出退出路径的确定性解释。
3. **【P1】「索引出错：未知」**：某卷 `state=error` 但 message 为空。已确认缺口：`inprocess.rs` 的 `load_volume_with` 出错时只设 `vd.state = VolumeState::Error`，**不记录错误信息**；`status_impl` 里 `message: None` 硬编码。需要打通错误信息链路。
4. **【P1】进度恒 0%**：`progress` 字段全仓库仅 `inprocess.rs` 中 `progress: None` 一处，从未赋值 → UI 进度永远 0。需要实现真实进度上报（IPC 节流到 100ms 级，禁止每文件推送）。
5. **【P2】持久化未接线**：`set_save_dir` 在 app 侧从未被调用 → M2 的 FFIX 落盘/载入实际从未生效，每次启动都全量重扫。确认这是遗漏还是有意设计。
6. UI 渲染正常（前端不是问题源头）；`cargo test -p ff-engine` 19 个单测全绿（但单测用 FakeScanner，覆盖不到真实卷路径）。

### 四、已修复项（不要重复修，但可复查是否正确）
1. debug/release 都走 `devUrl` 白屏：根因是 tauri build.rs 里 `dev = !custom_protocol`，已在 `app/src-tauri/Cargo.toml` 加 `custom-protocol` feature（release 必须带 `--features custom-protocol` 构建）。
2. 控制台窗口：`main.rs` 已改无条件 `#![windows_subsystem = "windows"]`。
3. 启动一闪即没：WebView2 子窗口获焦点导致父窗口收 `WM_ACTIVATE(WA_INACTIVE)` 被误判失焦，已加前台 PID 守卫（`foreground_pid()`）+ `focused_once` 守卫。

### 五、重点审查区域（按优先级）
1. **`crates/ff-ntfs/src/scanner.rs`（最高优先）**：`full_scan` 的 MFT 枚举循环——死循环风险、锁持有范围、单条记录解析失败的处理、缓冲区策略；`enumerate_volumes` 为何返回 5 个卷、无盘符卷为何 1–2ms「完成」（真快还是吞错）。检查 `ntfs-reader` 的版本与 API 用法（错误传播、重复读取、逐条 `read` 的 syscall 放大）。
2. **`crates/ff-engine/src/inprocess.rs`**：`rescan_impl` 持 `scanner` MutexGuard 跑完整扫描，`background_loop → poll_volume` 也要抢同一把锁 → 扫描期间后台线程整体阻塞（设计缺陷还是死锁风险？）；`load_volume_with`/`status_impl` 的 message/progress 缺口；`Drop` join 后台线程与进程退出的关系。
3. **进程退出路径**：结合「无 panic + 无 WER + 扫描途中死亡」，排查：栈溢出（path 重建递归？）、job object、tao 事件循环返回条件、外部杀软。给出可验证的结论。
4. **`app/src-tauri/src/main.rs`**：`build_engine` 后台线程里 `let _ = scan.rescan(...)` 吞错误；引擎状态→IPC→前端的错误传播链；临时 `dbg_log` 的去留建议。
5. 顺带：`IndexBuilder` 大卷内存占用（248k 文件量级估算）；`ff-cli` 是否有相同卡死（用 CLI 复现可以隔离 Tauri/WebView2 因素——**这是一个重要的隔离实验，请务必做**）。

### 六、环境约束（不遵守会浪费大量时间）
- **C: 盘满（0 GB 可用）**：cargo 构建前必须重定向临时目录，否则随机报「拒绝访问 (os error 5)」：
  ```bash
  export TMP=/f/Temp TMPDIR=/f/Temp TEMP=/f/Temp
  export CARGO_INCREMENTAL=0
  ```
- npm 不可用（缓存在 C:），测试 Rust 侧不需要 npm。
- 测试入口：`cargo test -p ff-engine`（19 个单测；新增测试沿用 FakeScanner 模式，不要依赖真实卷）。
- 复现卡死：按上述方式构建 release 并运行，观察 `%TEMP%\sift-debug.log` 是否出现 D: 卷的 `rescan done`（当前永不出现）。
- git：分支 main；M3 提交为 `4b65e04`；**工作树有未提交改动（上述已修复项 1–3 + dbg_log），审查基于工作树现状**。

### 七、交付要求
1. 每个结论带 `文件:行号` 证据与验证过程（命令/日志/实验）；
2. D: 扫描卡死的**第一性根因**（证据链完整，不接受"可能"）；
3. 修复清单按 P0（卡死/退出）→ P1（错误信息/进度）→ P2（设计问题：锁竞争/持久化未接线）排序，每个修复附验证方法；
4. 改动最小化，不顺手重构无关代码；
5. 最后量化回答：修复后 248k 文件全量扫描预期耗时、`cargo test -p ff-engine` 是否仍全绿、对 DESIGN §13-M3 验收（按键→首屏 ≤50ms）的影响。

## Prompt 正文（复制到这里结束）
