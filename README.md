# Sift — Everything 式本地文件搜索

一个驻留在系统托盘、用全局热键唤出的**毫秒级本地文件搜索工具**。首次为 NTFS 卷建立索引，之后通过 USN 日志增量追踪文件变化，重启直接复用磁盘索引（秒开）。支持子串 / 拼音 / 模糊匹配、查询语法、评分排序，以及 Tauri 2 + React 打造的无边框桌面 UI。

> Windows 为主战场（NTFS MFT + USN），非 Windows 仅保证编译通过。

## 它做什么

- **快**：MFT 全量扫描一次建索引，后续靠 USN 增量；重启复用磁盘索引，不再每次全扫。
- **多种匹配**：子串（`memchr`）、拼音、模糊（子序列），带评分排序。
- **查询语法**：`case:` / `nocase:` / `content:` / 正则 / 分组括号等。
- **随时唤出**：全局热键 `Ctrl+Alt+Space`，无边框窗口，失焦自动隐藏。
- **双后端**：普通权限走命名管道服务（`ff-service`），管理员权限回退到进程内引擎（InProcess）。
- **附加能力**：文件对话框路径回填（UIA）、可插拔本地 HTTP API。

## 快速开始

### 直接用（正式版）

到 [Releases](../../releases) 下载 `sift-app.exe`（或自行构建见下），双击运行。首次启动会对每个 NTFS 卷做一次全量索引（C: 卷实测约 85 万条目 / 22s），之后秒开。按 `Ctrl+Alt+Space` 唤出搜索框。

### 从源码构建

需要 Rust（stable）、Node.js 18+。

```bash
# 一条命令搞定（推荐）：构建前端 + 带 custom-protocol 的正式版
cd app && npm install && npm run build:release
#   产物：app/src-tauri/target/release/sift-app.exe

# 或手动分步（关键：必须带 custom-protocol，否则双击报"localhost 拒绝连接"）：
#   cd app && npm run build
#   cd src-tauri && cargo build --release --features custom-protocol
# 也可用 Tauri CLI（自动带该 feature）：npm run build:tauri
```

> ⚠️ 单纯的 `cargo build --release` **不带** `--features custom-protocol` 会产出白屏 exe（Tauri 走 `devUrl` 请求 `localhost:5173`）。开发调试用 `npm run dev:tauri`（需先起 vite dev server）。

## 使用

- **唤出 / 隐藏**：`Ctrl+Alt+Space`
- **搜索**：输入关键词，支持拼音与模糊；`↑`/`↓` 选择，`Enter` 打开。
- **查询语法**：`case:xxx` 区分大小写、`nocase:xxx` 忽略大小写、`content:xxx` 搜索文件内容、`/正则/`、`(a b)` 分组。
- **设置**：点界面「设置」，可配置开机自启、索引卷等；标注「未接线」的项尚未实现（界面不撒谎）。

## 项目结构

```
crates/
  ff-core/    纯逻辑内核：条目表/查询/AST/评分/内容搜索/持久化（forbid unsafe）
  ff-match/   匹配器：子串/拼音/模糊（forbid unsafe）
  ff-ntfs/    NTFS 卷读取：MFT 全扫 / USN 增量 / 非 NTFS 遍历回退
  ff-engine/  契约层 + InProcess 引擎 + Overlay + IPC 协议（核心）
  ff-dialog/  文件对话框识别与路径回填（UIA）
  ff-ipc/     命名管道（Windows）
  ff-http/    本地 HTTP API（默认关闭）
  ff-service/ Windows 服务外壳（SCM）
  ff-cli/     命令行调试：scan / search / import
app/
  src-tauri/  Tauri 2 后端（main.rs，11 个 command）
  src/        React 前端（App.tsx + 组件 + engine/）
bench/ tests/ 合成数据生成器 + 跨 crate / Windows 集成测试
docs/         设计 / 决策 / 基准 / UI 规范
HANDOVER/     接力包：状态、源码地图、打磨清单、验证、已知问题
```

## 验证（开发者）

```bash
# Rust 全工作区
cargo test --workspace
cargo clippy --workspace --all-targets

# 前端
cd app && npm run test:format && npm run build

# 真机集成测试（需管理员 + NTFS 卷）
cargo test -p sift-tests -- --ignored --nocapture
```

## 文档

- `docs/DESIGN.md` — 架构与契约
- `docs/DECISIONS.md` — 决策流水
- `docs/BENCH.md` — 真机基准数据
- `HANDOVER/README.md` — 接力 / 维护指南（改动前必读）
- `HANDOVER/05-KNOWN-ISSUES.md` — 已知问题与历史坑（含不要回退的关键修复）

## 状态

M0–M7 里程碑全部完成，进入打磨期。索引复用、启动性能、若干 panic/一致性缺陷已修复；剩余打磨项见 `HANDOVER/03-POLISH-BACKLOG.md`。
