# Sift 交接（2026-11-15，M6 已开始）

## 当前状态

- M0–M5 已完成。用户已认可当前 UI、授权提交并进入 M6。
- `e536db0`：搜索/设置 UI 重整、旧轮次窗口动效与 ACL 修复、规范及证据。
- `b9075e9`：设置轻量上下转场。用户强调风格一致，因此没有照搬 CompressO 的
  Vaul 抽屉、大圆角、拖拽把手或背景缩放；仅借鉴进入/退出节奏。
- **M6-1 已实现共享协议和时间边界，M6 整体仍进行中。** 当前应用仍为
  InProcess 引擎，没有切换服务，没有安装 SCM，没有开放 HTTP。

## 本批已落地（DECISIONS #86）

- `crates/ff-engine/src/ipc.rs`：版本化消息、六种方法、结构化错误、有界
  4 字节小端长度前缀 + UTF-8 JSON 编解码；每帧 1 MiB、查询 16 KiB、
  range 1000 上限与加法溢出校验。断线与截断区别处理，错误后应丢弃连接。
- `Open` 回复只携带解析行和 action；未来服务不能以 SYSTEM 身份执行
  打开文件/剪贴板动作，必须由 UI 执行（DESIGN §12）。
- `crates/ff-engine/src/timeconv.rs`：从 Tauri 移入，原 7 测试随迁移；
  Tauri 改用共享模块。Rust ↔ Rust 管道保留 FILETIME，最终 JS/HTTP 边界
  才转 epoch 毫秒，避免重复转换。不得在 TS 侧处理已丢精度的 FILETIME。
- `crates/ff-engine/tests/ipc_contract.rs`：11 项协议测试，先红后绿。
- 复用锁定 `serde_json 1.0.154` 为 ff-engine 直接依赖，无新网络/服务运行时。
- 细节以 `DESIGN`、`DECISIONS #85/#86`、`BENCH`、`DEPENDENCIES` 为准。

## 下一步顺序

1. **客户端会话隔离**：当前 `EngineInner::search_impl` 清掉全局 results/access，
   cancels 只保留最新 id。UI 与 HTTP 直接共用会互相作废；必须先隔离结果集、
   活跃搜索和取消令牌，同时共享索引，不为每个客户端复制全盘数据。
2. `ff-service`、SID ACL 命名管道、Pipe 客户端、断线提示/重连；管道超时、
   帧错误和权限用 Windows 实测，不把纯内存编解码当作 OS 管道验收。
3. SCM 安装/卸载与启动停止、普通权限 Tauri；打开文件仍由 UI 用户执行。
4. 默认关闭的 HTTP：127.0.0.1、Bearer token、严格 Host、无宽松 CORS、
   count/速率限制；错误 token/Host 等必须自动化测试。
5. 普通权限搜索、服务崩溃后提示与重连、HTTP 完整端到端验收后才标 M6 完成。
   不进入 M7，不自动改里程碑范围。

## 验证

- 根 workspace fmt/clippy：通过；test：**229 passed、5 ignored、0 failed**。
  包含从 Tauri 迁来的 7 时间测试与新 11 IPC 测试。
- Tauri fmt/clippy/test/custom-protocol check：通过；本地测试 0，
  不是丢失覆盖，时间测试在 ff-engine。
- 前端 build：50 modules；format：29 assertions；外置 Playwright UI 回归通过。
  转场检查涵盖动画名称、返回按钮焦点、Esc 保留查询与恢复输入焦点。
- **新 UI/转场尚未真实 WebView2 复验**。旧 #83 真机日志与截图只代表旧轮次。
  125% 缩放、IME、热键/失焦稳定性与原生窄窗口仍需覆盖。
- 浏览器预览 `http://127.0.0.1:5177/` 为 MockEngine，不是真实文件结果。
  UI 脚本 `app/scripts/check-ui.cjs` 的外部浏览器路径配置见 BENCH。

## 保留约束与欠账

- 不删除 120ms 搜索防抖；50ms 首屏指标不包含停顿窗口（#82）。
- 32px 虚拟行、UTF-16 高亮范围、四列表格、无左侧栏保持。
- 设置持久化、Shell 图标、列宽调整、Top-K 部分选择等仍有未完成项，
  不因 UI 改好或进入 M6 而误标完成。
- 窗口 CSS 不加常驻 will-change；避免多余大括号吞掉 topbar/flex 布局。
  Tauri 新增 core/event API 时检查 capabilities（#79/#83）。
- C: 空间约 1GB，Rust 构建设置 TMP/TEMP/TMPDIR 为 `F:/Temp`。
- 本轮不推送；已提交历史与当前状态以 `git log`、`git status` 为准。
