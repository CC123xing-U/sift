# 01 · 当前状态快照（2026-11-17）

> 对应提交：`6334be3`。事实来自 git log、真机日志与源码核对。**接手后请先按 `02-SOURCE-MAP.md`
> 复核，不要因本文件声明「已完成」就跳过验证。**

## 1. 里程碑：M0–M7 全部完成

| 里程碑 | 内容 | 状态 |
|---|---|---|
| M0 | 项目骨架、依赖登记、NTFS 读取可行性 | ✅ |
| M1 | 纯内存核心：SoA 条目表、路径现算、子串/拼音/模糊匹配 | ✅ |
| M2 | 卷扫描 + Base/Overlay 增量 + USN + 落盘/载入 | ✅ |
| M3 | 查询语言 M4 语法、AST 求值、分层匹配 | ✅ |
| M4 | 评分排序、Top-K（部分选择未做，见欠账） | ✅ |
| M5 | GUI 结果视图、右键菜单、拖拽、剪贴板、窗口动效 | ✅ |
| M6 | 命名管道 + ff-service + 普通权限 Tauri 双后端 + 本地 HTTP API | ✅ |
| M7 | 6 项：属性窗口 #91 / 对话框回填 #92 / 非NTFS回退 #93 / filelist导入 #94 / 内容搜索 #95 / 硬链接多路径 #97；另启动修复 #96 | ✅ |

决策流水在 `docs/DECISIONS.md`（#1–#97），每一项都记录了背景、方案、否决项与真机结论。
M7 收官于 `97b3b5c`。

## 2. 最近提交（git log 前几条，接手时以实际 git log 为准）

```
6334be3 docs: 收尾 HANDOVER 中 M7 残留的过时状态（六项收官，进入打磨期）  ← 本接力包之前的提交
97b3b5c feat(m7): 硬链接多路径（M7 第 6 项，里程碑收官）
2d9b798 fix: 启动窗口延迟显示与误藏（DECISIONS #96）
a38fbc5 feat(m7): 内容搜索 content:（M7 第 5 项）
9716db0 feat(m7): filelist 导入（M7 第 4 项）
8c9e1f9 feat(m7): 非 NTFS 回退遍历（M7 第 3 项）
4b30b7b feat(m7): 对话框集成（M7 第 2 项）
6dbd591 feat(m7): 属性窗口 Alt+Enter（M7 第 1 项）
```

## 3. 终态门禁（本包生成时实测）

- 根 workspace：`cargo test --workspace` → **325 passed / 0 failed**（生成当日复跑确认）；
  fmt / clippy `-D warnings` 历史全绿。
- Tauri（`app/src-tauri`）：fmt / clippy / test 全绿。
- 前端：`npm run build` 50 modules、`npm run test:format` 格式断言全绿。
- 依赖：`Cargo.lock` 长期未变，各阶段坚持零新增依赖。

## 4. 关键真机数据（历史峰值，供性能回归基线）

| 项目 | 数据 | 来源 |
|---|---|---|
| M5 排序 | 78,607 命中全量评分排序 110.3 ms | DESIGN §M5 边界 |
| M3/M4 全表 | 200 万条内存/搜索基准见 BENCH | bench/memprobe、m5probe |
| 内容搜索 content: | D: 268,669 条，`*.log content:error` 候选 138 / 扫 80 / 命中 27 / 二进制跳过 58 / 101 ms(release) | BENCH / #95 |
| 非 NTFS 回退 | exFAT VHD 9 条、CJK/嵌套正确、新建文件 30s 内可见 | BENCH / #93 |
| 硬链接多路径 | D: 270,317→279,854（+9,537 别名），不同名硬链接各自可搜且路径正确 | BENCH / #97 |
| 启动延迟修复 | `backend built` 5724ms→**2ms**，`setup begin` 6761ms→**568ms**，启动误藏 hide 事件 1→0 | BENCH / #96 |
| dialog 回填 | 标准「打开」框 UIA 读回 `MATCH=true` | BENCH / #92 |

## 5. 已知未完成的边界（不是 bug，是明确记账的欠账）

- **设置持久化**：`Settings.tsx` 全部状态仅存内存，11 项 UI 标注 pending/disabled（开机自启、
  失焦隐藏开关、双击 Ctrl、自定义热键、索引卷、排除路径、双拼方案、评分规则编辑器、
  HTTP API 配置、界面语言等）。见 `03-POLISH-BACKLOG` T1。
- **filelist 只有 CLI 入口**：`ff-cli import <列表> [查询]` 已实现，GUI 无导入入口、导入结果不持久化。
- **网络盘不支持**：`enumerate_volumes` 纳入可移动卷，但 `DRIVE_REMOTE` 遍历无超时/取消，未支持（DESIGN §5.4）。
- **拼音/模糊为演示级**：自实现精简字典（T2）+ 有序子序列（T3），未引入 `ib-pinyin` / `nucleo-matcher`，
  多音字未覆盖、双语字典未实现（DECISIONS #70 给了替换条件）。
- **Top-K 部分选择未做**：当前全量评分后全量排序，78,607 命中 110ms 尚未成为瓶颈。
- **用户规则未实现**：§8.3 的 FolderPriority / FileNamePriority / PriorFolders 与设置页评分规则编辑器仍禁用。
- **M4 边界**：`attrib:` 索引路径只判 R/H/S/D；通配符/正则不高亮。
- **ff-cli `bench`/`dump` 子命令**：留待后续里程碑（DECISIONS #40）。
- **单卷 rescan 粒度粗**：`rescan(volume)` 忽略 volume 参数、统一重建全部（`main.rs:236-239`）。
- **HTTP 只有 `GET /search`**：无 status/rescan 端点（需要时再扩展并重跑安全测试）。
- **真机未覆盖**：托盘/全局热键/失焦隐藏长期稳定性、中文 IME 组词、SCM 服务的跨会话 ACL 与崩溃重连
  （M6 在管理员会话验收过一次后已卸载服务）。
- **文档滞后实证**：`crates/ff-engine/src/lib.rs:6-9` 注释与实现矛盾（详见 `05-KNOWN-ISSUES` #1）。

## 6. 数据与环境注意

- C: 空间约 1GB，Rust 构建请设 TMP/TEMP/TMPDIR = `F:/Temp`。
- 索引落盘目录：程序所在磁盘根 `\Sift\`，文件名 `<serial>.ffix`（被 .gitignore 排除，勿入库）。
- 开发目录 `D:\Sift` 恰是仓库根，`.ffix` 不会被 git 跟踪——这是有意的。
- 调试日志：`%TEMP%\sift-debug.log`（`dbg_log`，注释标「定位后移除」，>1MB 截断），属打磨清理项。
