# 04 · 验证、真机步骤与踩坑手册

> 每次改动前后按本文件跑门禁。**所有命令均在 `D:/Sift` 下执行**（前端部分进 `app/`）。
> C: 空间紧张，Rust 构建把 TMP/TEMP/TMPDIR 指向 `F:/Temp`：
> ```bash
> export TMP=F:/Temp TEMP=F:/Temp TMPDIR=F:/Temp
> ```

## 1. 门禁命令（双 workspace + 前端）

根 workspace：
```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace            # 当前基线 325 passed / 0 failed
```

Tauri（独立 workspace，必须单独跑，不在根 workspace 成员里）：
```bash
cd app/src-tauri
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo check
cargo test                        # timeconv 边界测试
```

前端：
```bash
cd app
npm run build                     # tsc -b && vite build，当前 50 modules
npm run test:format               # 零依赖格式断言（format.ts / i18n.ts）
# 可选（需外部 Playwright，仅布局仿真，不验原生 IPC/热键/窗口）：
npm run test:ui                   # 需先起 dev server 在 127.0.0.1:5177
```

**验收基线**：改动后这些命令全绿、测试数不低于改动前（若新增功能应有新增测试）。
当前 workspace 基线 **325 passed / 0 failed / 5 ignored**（ignored 为 Windows 真卷集成测试）。

## 2. 真机验证步骤（Windows，部分需管理员）

### 索引与搜索（ff-cli，最快反馈）
```bash
cargo run -p ff-cli -- scan C:            # 全量扫描计时（管理员，直读 MFT）
cargo run -p ff-cli -- search D: "关键字"  # InProcess 建索引 + 查询
cargo run -p ff-cli -- import <列表文件> [查询]   # filelist 导入（跨平台纯逻辑）
```
- 关注：条目数、耗时、`文件系统 xxx（...）` 提示（exFAT 应显示「目录遍历回退（无增量）」）。

### 内容搜索 content:（M7，见 #95）
- 造小样本：`*.log content:error`、大文件（>32MiB 应跳过并计入 `ContentReport.truncated`）、
  二进制文件（应跳过）、`case:` 敏感、无名字条件（应报带 span 的解析错误）。
- 真卷基线：D: 268,669 条 `*.log content:error` → 候选 138 / 扫 80 / 命中 27 / 二进制跳过 58 / 101ms(release)。

### 硬链接多路径（M7，见 #97）
- `fsutil hardlink create` 建两个不同名硬链接 → `ff-cli scan` 条目数增加 → 分别搜主名/别名，路径应正确 →
  删别名重扫，别名 0 命中而主路径仍在。

### 属性窗口 / 对话框回填（M7，见 #91/#92）
- 属性：选中结果 `Alt+Enter` → 弹出系统属性框（真机踩坑：必须用 ShellExecuteExW + SEE_MASK_INVOKEIDLIST）。
- 回填：打开任意标准「打开」框 → Sift 出现对话框提示 → 选中结果 `Alt+D` → UIA 读回路径 `MATCH=true`
  （真机踩坑：裸 WM_SETTEXT 对 IFileDialog 无效，必须 UIA ValuePattern）。

### 窗口 / 托盘 / 热键（Tauri，需 GUI）
- 启动：窗口应立即显示（#96 后 `backend built ~2ms` / `setup begin ~568ms`），不应先出阴影再等索引。
- 全局热键 `Ctrl+Alt+Space` 唤出/隐藏；失焦自动隐藏；托盘菜单 show/hide/rescan/quit。
- 调试日志：`%TEMP%\sift-debug.log`（启动流程/焦点链路排查用）。

### 服务模式（M6，需管理员，实测后记得清理）
```bash
# ff-service 的 SCM 命令（install/uninstall/start/stop/status）
cargo run -p ff-service -- install
cargo run -p ff-service -- start
# 普通权限 UI 应自动走命名管道（\\.\pipe\sift-<SID>）
cargo run -p ff-service -- stop
cargo run -p ff-service -- uninstall
# 清理 http.json / 索引 / 探测程序（见 docs/HANDOVER M6 段）
```

## 3. 已知踩坑与不要回退的修复

| 坑 | 现象 | 正确做法 | 位置 |
|---|---|---|---|
| 窗口隐藏代数写成 `==gen` | 延迟隐藏 100% 被自己作废，窗口关不掉 | 用 `HIDE_GEN.load() == gen + 1` | `main.rs:374`（#83） |
| 搜索同步执行 | content: 读盘阻塞 IPC 线程，窗口「未响应」 | `async fn` + `spawn_blocking` | `main.rs:186-190` |
| 前台 PID 瞬态为 0 当失焦 | 启动 2.7s 即自动隐藏，只看到阴影 | `fg_pid==0` 时永不隐藏 + 重试 3 次 | `main.rs:416,763`（#96） |
| load_all 在窗口创建前同步跑 | ~5s 卡住窗口创建，延迟显示 | 重活全移入后台线程 | `main.rs:503-553`（#96） |
| 仅加 `data-tauri-drag-region` | 真机报 `start_dragging not allowed by ACL` | 同步补 capabilities/default.json | `capabilities/default.json`（#79） |
| Overlay 空名条目直接写回 | USN 记录无名字，空名遮蔽 Base 命中、污染落盘 | 按 frn 从 Base 查回原名 + 标 stale_meta | `ff-engine/overlay.rs` |
| AB-BA 死锁 | volumes.read 持锁抢 scanner 锁，反向同理 | 三阶段分离，任一时刻至多一把锁 | `inprocess.rs:620-626` + 回归测试:2792 |
| `ShellExecuteW("properties")` | 返回 31 误判失败 | `ShellExecuteExW + SEE_MASK_INVOKEIDLIST` | `ff-engine/actions.rs`（#91） |
| 裸 `WM_SETTEXT` 填对话框 | 对 IFileDialog 无效（1148 是 ComboBoxEx32） | UIA `ValuePattern::SetValue` | `ff-dialog/win.rs`（#92） |
| 大文件部分读取 content: | 开头命中/尾部不命中不一致 | 按 metadata 体积一律跳过 | `ff-core/content.rs` |
| heredoc 写硬链接 example 吞反斜杠 | `InvalidFilename` | 用 Write 工具写文件而非 heredoc | 历史踩坑 |
| 无增量的卷从不重走 | exFAT 新建文件搜不到 | 引擎每 30s 全量重走 | `ff-engine/inprocess.rs`（#93） |

## 4. 写入证据与文档

- 每次真机实测数据写入 `docs/BENCH.md`（按里程碑小节）。
- 设计/契约变化写入 `docs/DESIGN.md`，决策（含否决项）追加 `docs/DECISIONS.md` 新编号。
- 新依赖先登记 `docs/DEPENDENCIES.md`；本打磨期尽量零新增依赖。
- UI 变化同步 `docs/UI_SPEC.md`；截图/证据放 `docs/evidence/`。
- 本接力包自身的修订直接改 `HANDOVER/*.md`，发现问题追加 `05-KNOWN-ISSUES.md`。
