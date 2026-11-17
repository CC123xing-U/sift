# 03 · 打磨任务卡（欠账清单）

> 每条任务卡给出：现状（源码实证）、涉及文件、验收标准、优先级与依赖。
> **接手 AI 请先按 `02-SOURCE-MAP.md` 复核现状再动手**——这些「现状」是 2026-11-17 的快照。
> 状态标记：`[ ]` 未做 / `[~]` 部分 / `[x]` 已完成（接手后请更新）。

---

## T1 [ ] 设置持久化（最高用户可感知价值）

- **现状**：`app/src/components/Settings.tsx` 的 `DEFAULT_SETTINGS`（:20-32）与 `SettingsState`（:5-18）
  全是 React 内存态，文件头注释明写「当前仅存内存（跨视图切换保留）…落盘待 M6」（:4）；M6 已过仍未做。
  11 项 pending/disabled（见 T2）。热键、排除路径、列设置等一旦重启全部丢失。
- **涉及文件**：`Settings.tsx`（状态模型 + 落盘时机）、`App.tsx:64-69`（settings state 初始化）、
  可能新增 Tauri command（`app/src-tauri/src/main.rs`）读写配置文件；配置路径建议 `%APPDATA%\Sift\`（注意 index 在盘根 `\Sift\`）。
- **验收**：改设置→退出→重启 UI，设置保持；损坏/缺失配置文件有默认值兜底不崩；不把敏感信息明文落盘（如有 token）。
- **依赖**：这是 T2 大部分子项的前置——「接线」之前得有地方存。可与 T2 合并推进。
- **优先级**：高。用户每次启动都要重设，是最明显的「没打磨」。

### T2 [ ] 设置页 11 项 pending 接线（依赖 T1）

`Settings.tsx` 逐条（状态实证，行号为快照）：

| 子项 | 位置 | 现状 | 接线要点 |
|---|---|---|---|
| 开机自启 | `:253` | `pendingNote="M6 提供"` | Windows 注册表 Run 项或任务计划；需 Tauri command + 权限 |
| 失焦自动隐藏开关 | `:259` | `pendingNote="开关未接线"` | Rust `on_window_event` 恒开（main.rs:747-779），需加状态位 + 前端开关 |
| 双击 Ctrl 唤出 | `:265` | `pendingNote="未实现"` | 需在 Rust/前端做双击检测 + 注册热键 |
| 自定义全局热键 | `:134,139-143` | 录制 UI 可点但 `disabled`：「自定义未接线」 | 需动态重注册快捷键（当前硬编码 `CmdOrCtrl+Alt+Space:699`） |
| 索引卷选择 | `:272-275` | `<fieldset disabled>`「未接线」 | 需引擎暴露卷列表 + 选择性 rescan（也解 T7 粒度） |
| 排除路径 | `:175-178` | `disabled`「未接线」 | 需引擎排除集接线（ff-ntfs 已有 EXCLUDED_DIR_NAMES 常量，需支持用户自定义路径） |
| 双拼方案 | `:304-305` | `disabled`「暂未支持」 | 拼音层（ff-match/pinyin.rs）本就精简字典，属大工程，建议后置 |
| 多音字 / 双语字典 | — | 未覆盖（DECISIONS #70） | 替换 `ib-pinyin` 的时机，需评估依赖与体积 |
| 评分规则编辑器 | `:315-325` | `disabled`「M5 预留，未实现」 | §8.3 用户规则（FolderPriority/FileNamePriority/PriorFolders）引擎侧也未实现 |
| HTTP API 配置 | `:327-336` | `disabled`「M6 提供」 | `ff-http` 已有 http.json 开关，缺 UI 读写与重启生效 |
| 界面语言 | `:342-345` | `disabled`「仅中文」 | `i18n.ts` 只有中文资源，需补英文 + 切换机制 |

- **注意 UI 设计约定**（`Settings.tsx:70-74,99-102`）：未接线项必须显式 disabled + 中文标签，
  不得以本地状态冒充生效。接线完成一项摘一项的标注，不要一次性全放开。
- **验收**：每项接通后有真实行为（不是只改界面状态）；关闭/开启生效；与 T1 持久化联动。

---

## T3 [ ] filelist 的 GUI 入口 + 导入结果持久化

- **现状**：`ff-cli run_import`（`crates/ff-cli/src/main.rs:406`）与 `ff-core/src/import.rs`（`parse_list:89`
  `build_indexes:463`）已完整实现，纯路径 + Everything 风格 CSV，真机 4 行→10 条（#94）。
  但**没有任何 GUI 入口**，导入结果不入盘，重启即失。
- **涉及文件**：需新增文件选择对话框（可复用 `ff-dialog` 的 UIA 能力，或用 Tauri 原生 dialog——注意 capabilities）、
  新增 Tauri command 调 `ff_core::import`、引擎侧把导入卷纳入索引/落盘、`App.tsx` 入口、i18n 文案。
- **验收**：UI 选一个列表文件→导入→立即可搜→重启后仍在；CSV 引号内逗号、非 UTF-8 报错与 CLI 一致。
- **依赖**：涉及 FFIX 格式（`FORMAT_VERSION=1`），导入卷如何持久化需设计——尽量复用现有 `<serial>.ffix`，
  import 用最高位 serial 与真实卷隔开（import.rs 已有 `IMPORT_SERIAL_FLAG` 语义，复核现状）。
- **优先级**：中高。CLI 已证明可用，缺的只是「 last mile 」到 UI。

## T4 [ ] 列宽调整 / 结果视图打磨

- **现状**：四列表格列宽固定（DESIGN/HANDOVER 约束「32px 虚拟行、四列表格、无左侧栏保持」），
  `ResultArea.tsx` 用 `@tanstack/react-virtual`。列头可切升/降序（:315-353），但列宽不可拖。
- **涉及文件**：`ResultArea.tsx`、`styles.css`（约 1107 行）、可能改列宽状态（ → T1 持久化）。
- **验收**：拖动列分隔线改变列宽；持久化（接 T1）；不影响虚拟化与 32px 行高；窗口缩放自适应。
- **优先级**：中。属于「好用」层面的打磨。

## T5 [ ] Top-K 部分选择（性能优化）

- **现状**：`ff-core/src/score.rs::search_ranked:261` 当前对全部命中评分后全量排序。
  DESIGN §M5 明确「§8.2 的 Top-K 部分选择（`select_nth_unstable_by`）未做」。
  78,607 命中实测 110.3 ms，**尚未成为瓶颈**。
- **涉及文件**：`ff-core/src/score.rs`、相关单测、`docs/BENCH.md`（性能回归数据）。
- **验收**：结果正确的前提下，大结果集（10 万+）排序耗时下降；小结果集无回归；单测全绿。
- **优先级**：低-中（非瓶颈，但 easy win 且是明确的未完成项）。

## T6 [ ] Shell 图标 / 文件类型图标

- **现状**：`icons.tsx` 是 7 个线性 SVG 图标；结果行 `fileCategory`/`FolderIcon`/`FileIcon` 按类别粗分，
  无真实 Shell 关联图标。`docs/ui-reference/` 下有参考。
- **涉及文件**：`ResultArea.tsx`（`RowContent`/`fileCategory`）、`icons.tsx`、可能需 Tauri/Win32 取 SHGetFileInfo。
- **验收**：常见文件类型显示对应图标；无图标的类型优雅降级；不显著拖慢列表渲染（缓存！）。
- **优先级**：低。观感项。

---

## T7 [ ] 清理与正确性（低风险，随手机动）

- [ ] **`ff-engine/src/lib.rs:6-9` 过期注释**：声称 Base+Overlay/落盘/USN 未落地，实际已实现。
      改为准确描述；确认「后台合并 compaction」是否仍未做（源码未见 compaction 逻辑）——若确实没做，
      单独记一条欠账，不要整段删。见 `05-KNOWN-ISSUES #1`。
- [ ] **常驻调试日志**：`app/src-tauri/src/main.rs:442-470` `dbg_log` 注释标「定位后移除」，
      写 `%TEMP%\sift-debug.log`。改为可关（配置/环境变量/debug_assert）或移除，避免长期写盘。
- [ ] **单卷 rescan 粒度**：`main.rs:234-239` 忽略 `volume` 参数统一重建全部。需引擎暴露卷列表
      （`ff-cli` 已有 find_volume 逻辑可参考），实现真正的单卷重建。与 T2「索引卷选择」关联。
- [ ] **`has_active_dialog` 命令无调用点**：`main.rs:335,788` 已注册，前端无人用（提示改由事件驱动）。
      要么接上、要么删掉，消除死代码。见 `05 #12`。
- [ ] **`format.ts:6` 文档路径失配**：注释指向不存在的 `app/src-tauri/src/timeconv.rs`，实际在 `ff-engine`。
- [ ] **前端无测试框架**：`scripts/check-format.mjs` 是零依赖临时方案，`test:ui` 仅布局仿真不验原生 IPC。
      若打磨需要，可引入 Vitest（注意「零新增依赖」约束，需权衡）。

## T8 [ ] 网络盘支持（明确记录的不支持，大工程）

- **现状**：DESIGN §5.4 与 HANDOVER 记 `DRIVE_REMOTE` 不支持——目录遍历无超时/取消。
  `ff-ntfs::win32::enumerate_volumes` 纳入可移动卷但未纳入网络盘。
- **涉及文件**：`ff-ntfs/src/{win32.rs,auto.rs,walk.rs}`；需给遍历加超时/取消令牌（ff-core::CancelToken 可复用）。
- **验收**：挂载网络盘可扫描、可中途取消、断线不卡死。
- **优先级**：低（用户核心场景是本地盘）。

## T9 [ ] 真机验收补齐（质量信心）

- M6 服务化在管理员会话验收一次后已卸载（http.json/索引/探测程序已清理）；SCM 跨会话 ACL、服务崩溃重连
  未做长期验证。
- 未覆盖：托盘 / 全局热键 / 失焦隐藏长期稳定性、中文 IME 组词（DESIGN §M5 诚实记录）。
- 建议：每个打磨项完成后按 `04-VERIFY.md` 跑门禁 + 相关真机步骤，新数据写入 `docs/BENCH.md`。

---

## 排期建议（非强制）

1. **第一刀**：T1 + T2（设置持久化 + 接线）——价值最高，注意分批接线、逐项摘标注。
2. **第二刀**：T3（filelist GUI）——需先定导入卷持久化方案，可能与 T1 的配置文件机制协同。
3. **第三刀**：T4/T5/T7（视图与清理）——低风险打磨。
4. **按需**：T6/T8/T9。

每完成一项：更新本文件状态标记 → 跑 `04-VERIFY.md` 门禁 → 在 `05-KNOWN-ISSUES.md` 追加新发现 →
git commit（信息带任务编号，如 `polish(T1): 设置持久化`）。
