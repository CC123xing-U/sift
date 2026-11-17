# Sift 项目交接包（接力 AI 必读）

> 生成日期：2026-11-17 · 对应代码提交：`6334be3` · 项目状态：**M0–M7 全部完成，进入打磨期**
> 本目录是给「接手做深入优化打磨的 AI」准备的接力材料。原项目文档位于 `../docs/`。

---

## 0. 接手第一指令（务必先做）

**不要只读文档就开始改代码。** 本包与 `docs/` 下的文档都是「地图」，不是「领土」——
它们可能滞后、可能自相矛盾、可能把计划当成已实现。已知实证：`crates/ff-engine/src/lib.rs:6-9`
的 crate 注释至今仍写「Base+Overlay 增量层、索引落盘、USN 增量接线仍未落地、`last_saved` 恒为
None」，但 `inprocess.rs` 早已实现 `save_all`/`load_all`/Overlay/USN 轮询并真机验收。
**文档会骗人，源码不会。**

因此，正式动手前按 `02-SOURCE-MAP.md` 的「逐文件复核清单」把相关源文件重新打开看一遍，
按 `03-POLISH-BACKLOG.md` 的任务卡逐条对照代码现状，再决定改什么。发现文档与源码冲突时，
**以源码为准**，并顺手把冲突记进 `05-KNOWN-ISSUES.md`（本包欢迎修订）。

## 1. 这个包怎么读

| 文件 | 内容 | 何时读 |
|---|---|---|
| `README.md`（本文件） | 交接总览、硬约束、术语、环境与门禁速查 | 第一个读 |
| `01-STATUS.md` | 当前状态快照：已交付里程碑、真机数据、提交历史、未完成边界 | 第二个读 |
| `02-SOURCE-MAP.md` | 逐 crate / 逐文件的源码地图 + **逐文件复核清单** + 关键 API 行号 | 动手前必读，并据此重新审阅源码 |
| `03-POLISH-BACKLOG.md` | 打磨任务卡：每条欠账的现状、涉及文件、验收标准、优先级建议 | 选任务时读 |
| `04-VERIFY.md` | 门禁命令、真机验证步骤、已知踩坑与不要回退的修复 | 每次改动前后读 |
| `05-KNOWN-ISSUES.md` | 源码实证的问题点（含文档滞后、panic 风险、历史坑、待核实项） | 全程参照，发现问题追加 |

原始设计/决策/证据（**权威细节在源码，这些是背景**）：
`docs/DESIGN.md`（架构与契约）、`docs/DECISIONS.md`（#1–#97 决策流水）、
`docs/BENCH.md`（每一次真机实测数据）、`docs/UI_SPEC.md`（UI 规范）、
`docs/HANDOVER.md`（旧版交接，本次已同步）、`docs/DEPENDENCIES.md`（依赖登记）。

## 2. 项目一句话

Sift 是一个 **Everything 式的本地文件搜索工具**：Rust 工作区负责 NTFS MFT 全量扫描 +
USN 增量、多层名字匹配（子串/拼音/模糊）、查询语言、评分排序、命名管道服务 + 本地 HTTP API；
Tauri 2 + React 做无边框、全局热键唤出的桌面 UI。Windows 为主战场，非 Windows 仅保证编译。

## 3. 仓库布局

```
D:/Sift/
├── Cargo.toml              # 根 workspace：9 个 ff-* crate + bench + tests
├── crates/
│   ├── ff-core/            # 纯逻辑内核：条目表/查询/AST/评分/内容搜索/导入/持久化（forbid unsafe）
│   ├── ff-match/           # 匹配器：子串(memchr)/拼音(精简字典)/模糊(子序列)（forbid unsafe）
│   ├── ff-ntfs/            # NTFS 卷读取：MFT/USN/非NTFS遍历/自动分派（不依赖 ff-core）
│   ├── ff-engine/          # 契约层 + InProcess 实现 + Overlay + IPC 协议（最大 crate）
│   ├── ff-dialog/          # M7 文件对话框识别与回填（UIA）
│   ├── ff-ipc/             # M6 命名管道（整 crate cfg(windows)）
│   ├── ff-http/            # M6 本地 HTTP API（纯 std 手写，默认关闭）
│   ├── ff-service/         # M6 Windows 服务外壳（SCM）
│   └── ff-cli/             # 命令行调试：scan/search/import
├── app/
│   ├── src-tauri/          # Tauri 后端，仅 main.rs 一个源文件（11 个 command）
│   └── src/                # React 前端（App.tsx + 4 组件 + engine/ + i18n + types）
├── bench/ tests/           # 合成数据生成器 + 跨 crate / Windows 集成测试
├── docs/                   # 设计/决策/基准/UI/旧交接
└── HANDOVER/               # 本接力包
```

## 4. 硬约束（改动时不得违反）

- **契约冻结**：`ff-engine::api`（EngineClient 六方法）、IPC 帧格式、FFIX 格式 version 1、
  `ff-ntfs` 的 `RawEntry`/`Change`、TS 侧 `types.ts` —— 改动牵一发动全身，先读 `docs/DESIGN.md`
  对应章节再动。`FORMAT_VERSION` 目前为 1，不要在没有迁移方案时提升。
- **平台隔离**：ff-core / ff-match 声明 `#![forbid(unsafe_code)]`；平台相关 unsafe 集中在
  ff-ntfs/ff-ipc/ff-dialog/ff-service 的 `#[cfg(windows)]` 里。非 Windows 必须仍能编译。
- **「界面不撒谎」**：所有未接线的设置项都显式 `disabled` 并带中文 pending 标签
  （`Settings.tsx`）。新增或接线功能时同步摘掉标注，反之不得让 UI 假装生效。
- **双后端语义**：Tauri 启动先试命名管道（普通权限→服务），失败回退 InProcess（管理员）。
  `open` 动作在服务模式下只返回 `ResolvedAction`，必须由 UI 进程执行（DESIGN §12）。
- **不要回退的关键修复**：窗口隐藏代数 `gen+1`（`main.rs:374`，否则延迟隐藏 100% 被自己作废）、
  搜索 `async + spawn_blocking`（`main.rs:186-190`，否则 `content:` 读盘冻结窗口）、
  失焦守卫 `fg_pid==0` 不隐藏（`main.rs:763`，否则启动即误藏）、
  overlay 空名回填（USN 记录无名字，须按 frn 从 Base 查回原名）。详见 `04-VERIFY.md`。
- **无新增依赖优先**：历史各阶段都强调零新增依赖（Cargo.lock 长期未变）。引入新 crate 前先查
  `docs/DEPENDENCIES.md` 与 DECISIONS #70（拼音/模糊库的替换条件）。

## 5. 环境与门禁速查

- 工作目录 `D:/Sift`；C: 空间紧张（约 1GB），Rust 构建把 TMP/TEMP/TMPDIR 指向 `F:/Temp`。
- 两个 workspace：根（`cargo test --workspace`，当前 **325 passed / 0 failed**）、
  `app/src-tauri`（独立 workspace，需单独 fmt/clippy/test）。
- 前端：`cd app && npm run build`（50 modules）、`npm run test:format`（零依赖格式断言）。
- 完整命令与真机步骤见 `04-VERIFY.md`。

## 6. 打磨期建议起点（详见 03-POLISH-BACKLOG）

按「用户可感知价值 × 实现成本」粗排，供选任务参考（非强制顺序）：
1. **设置持久化**（ biggest gap：Settings 全是内存态，11 项 pending）——用户每次启动都要重设。
2. **filelist UI 入口 + 导入结果持久化**——`ff-cli import` 已可用但 GUI 无入口。
3. **列宽调整 / 结果视图打磨**——四列表格固定列宽。
4. **Top-K 部分选择**（`select_nth_unstable_by`）——大结果集性能优化，当前非瓶颈。
5. **Shell 图标 / 文件类型图标**。
6. 清理类：`ff-engine lib.rs` 过期注释、把常驻调试日志 `dbg_log` 改为可关、单卷 rescan 粒度。

接手 AI 可自行重新排序，但每完成一项请更新对应任务卡状态、跑门禁、并在 `05-KNOWN-ISSUES.md`
记录新发现，最后 git commit（提交信息带打磨项编号）。
