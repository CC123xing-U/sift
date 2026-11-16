# Sift 交接（2026-11-16，M6 已开始）

## 当前状态

- M0–M5 已完成。用户已认可当前 UI、授权提交并进入 M6。
- `e536db0`：搜索/设置 UI 重整、旧轮次窗口动效与 ACL 修复、规范及证据。
- `b9075e9`：设置轻量上下转场。用户强调风格一致，因此没有照搬 CompressO 的
  Vaul 抽屉、大圆角、拖拽把手或背景缩放；仅借鉴进入/退出节奏。
- **M6-1/M6-2/M6-3 + 命名管道与服务外壳已实现，M6 整体仍进行中。** 当前应用仍为
  InProcess 引擎，没有切换到 Pipe 客户端，没有安装 SCM，没有开放 HTTP。
- 新增 `ff-ipc`（SID ACL 命名管道 server/client）与 `ff-service`（SCM 服务外壳），
  真实 OS 管道端到端测试通过；下一对话从「普通权限 Tauri 接 PipeClient + 重连」开始。

## 上一轮局部调整（2026-11-16，已随本轮提交）

- `app/src-tauri/src/main.rs`：桌面版索引目录改为运行程序所在磁盘根目录的
  `Sift\`；启动时按当前卷 serial 清理旧 `.ffix`，并清理旧
  `%LOCALAPPDATA%\Sift\` 中的 `.ffix`。同一卷仍用固定文件名原子覆盖。
- `app/src-tauri/tauri.conf.json`、`app/src/styles.css`：无边框窗口使用透明底与
  CSS 圆角，调整窗口壳层曲线。
- `app/src/App.tsx`、`app/src/styles.css`：设置页进入/退出与返回主界面采用统一的
  260ms 纵向过渡；退出兜底计时为 280ms。
- `app/src/components/Settings.tsx`：索引位置文案改为运行盘根目录语义。
- `docs/DESIGN.md`、`docs/DECISIONS.md`：同步索引目录决策。
- 这些改动连同下方 M6-2/M6-3 与本轮修复已一并提交（见 `git log`）。

## M6-2 客户端会话隔离（已完成）

- `crates/ff-engine/src/inprocess.rs`：新增 `SessionState` 与 `EngineSession`。
  `InProcessEngine::create_session()` 创建独立客户端会话；每个会话分别维护
  结果集、60 秒访问 TTL 和取消令牌，索引卷数据仍由所有会话共享。
- 默认 `InProcessEngine` 保留 session 1，现有 Tauri 调用不需要改接口；未来 Pipe/HTTP
  客户端应为每条连接创建一个 `EngineSession`，连接关闭时自动释放该会话状态。
- 同一会话的新搜索仍清理该会话的旧结果；不同会话之间不会互相作废或取消。
- 回归测试：`client_sessions_keep_results_and_cancellation_isolated`。

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
- `ff_engine::ipc::serve_connection`：复用有界帧编解码，按连接循环处理 Search/Range/
  Cancel/Status/Open；Open 只返回解析行，不执行 UI 动作；Rescan 由调用方注入卷解析器。
  新增连接循环回归测试，IPC 合约测试现为 12 项。
- 复用锁定 `serde_json 1.0.154` 为 ff-engine 直接依赖，无新网络/服务运行时。
- 细节以 `DESIGN`、`DECISIONS #85/#86`、`BENCH`、`DEPENDENCIES` 为准。

## 已完成阶段确认错误修复（2026-11-16 接手轮，已提交）

二次确认原审查报告后，忽略未完成阶段（E1 多会话 searching、M7 非 NTFS/内容搜索、
设置持久化等）与有意设计（C1/C2 拼音字典演示级 #70、C4 CLI `join(" ")` #28、
C6 glob `?` 按字节 §8.1），仅修 M0–M5 已完成阶段的确认错误：

- **S1/S2（数据损坏，P0）** `ff-engine/overlay.rs`：`Overlay::apply` 增 `base: &VolumeIndex`
  参数。USN 内容/元数据变化记录不带名字（`RawEntry.name` 为空串），按 frn 从 Base 查回
  原名，避免空名条目遮蔽 Base 命中、固化进落盘索引；空名条目标 `stale_meta` 使 UI 显示
  占位而非 "0 B"。两处调用点（锁内增量、锁外合并）均传 Base。回归测试
  `overlay_nameless_upsert_preserves_base_name_and_marks_stale`。
- **E7** `overlay.rs`：删除全仓无引用的 `OverlaySet` 别名。
- **E5** `inprocess.rs::load_all`：仅实际载入 `.ffix` 时才置 `last_saved`，全新安装不再
  显示误导性"已保存"时间。
- **E6** `inprocess.rs::to_utf16_ranges`：钳制越界字节端点、丢弃非法区间，防高亮 panic；
  追加越界/非法区间断言。
- **C3** `ff-match/pinyin.rs`：`\u{ffff}` 占位符改为子序列屏障，阻止 `zr` 跨未收录汉字误连；
  回归测试 `placeholder_blocks_cross_character_subsequence`。
- **S3** `main.rs::build_engine`：卷枚举失败时跳过 prune，不再用空集删光全部 `.ffix`。
- **S4** `main.rs::rescan_all`：托盘重建改 spawn 线程，与命令路径一致，不再冻结 UI。
- **L2** `main.rs::run`：全局快捷键注册失败优雅降级（记日志继续），不再 `expect` 崩溃。

验证：Rust workspace + Tauri 三道门禁（fmt / clippy -D warnings / test）全绿，
0 失败（ff-engine 43+12 IPC、ff-match 25、ff-core 88）；前端 50 模块 build + 29 项格式
断言通过；CLI 真实扫描 C: 盘 846090 条、搜索 `ntdll` 命中并输出正常。
L3（Alt+F4 退出终止托盘）涉及退出语义、DESIGN/DECISIONS 未规定，未擅改。

## M6 第 1 项：命名管道 + 服务外壳（2026-11-16，已实现）

- `crates/ff-ipc`（新）：SID/SDDL ACL、HANDLE→Read/Write、server accept 循环（每连接
  `EngineSession` + 独立线程）、`PipeClient`（`EngineClient over Pipe`）。协议编解码仍只在
  `ff-engine::ipc`。管道名 `\\.\pipe\sift-<用户 SID>`，SDDL 只放行登录用户 + SYSTEM。
- `crates/ff-service`（新）：windows-service 服务外壳；`install/uninstall/start/stop/status`
  SCM 命令；服务以 LocalSystem 运行，用 `WTSQueryUserToken` 取活动控制台用户 SID 建管道；
  索引放 `%ProgramData%\Sift`；Stop/Shutdown 时保存索引并上报状态。
- 新依赖：`windows-service 0.8.1` + windows 新增 pipes/security/remotedesktop features，
  已登记 DEPENDENCIES.md（DESIGN §88）。
- **未实测**：SCM 实际安装/启停（需管理员）、SYSTEM 跨会话 ACL、服务崩溃重连；
  这些要在真机以普通权限 UI + 已安装服务验收，当前只做了进程内真实 OS 管道往返。

## 下一步顺序

1. **普通权限 Tauri 接 `PipeClient`**：UI 启动时判断是否已安装/运行服务，连
   `\\.\pipe\sift-<SID>`；打开文件/剪贴板仍由 UI 用 `PipeClient::last_open()` 的结果以自身
   身份执行。实现服务崩溃提示与自动重连（当前连接只在启动时建立，断开后无重连）。
2. 服务崩溃后完整重连验收；SCM 安装/启停、SYSTEM 跨会话 ACL 真机实测（需管理员）。
3. 默认关闭的 HTTP：127.0.0.1、Bearer token、严格 Host、无宽松 CORS、
   count/速率限制；错误 token/Host 等必须自动化测试。
4. HTTP 完整端到端验收后才标 M6 完成。不进入 M7，不自动改里程碑范围。

## 验证

- 根 workspace fmt/clippy：通过；test：**229 passed、5 ignored、0 failed**。
  包含从 Tauri 迁来的 7 时间测试与新 11 IPC 测试。
- Tauri fmt/clippy/test/custom-protocol check：通过；本地测试 0，
  不是丢失覆盖，时间测试在 ff-engine。
- 前端 build：50 modules；format：29 assertions；外置 Playwright UI 回归通过。
  转场检查涵盖动画名称、返回按钮焦点、Esc 保留查询与恢复输入焦点。
- 本轮局部调整后的验证：Rust workspace/Tauri `custom-protocol` 检查通过，前端
  `npm run build` 与 `npm run test:format`（29 assertions）通过；Playwright 回归
  未运行（当前环境缺少 `playwright` 模块）。透明圆角、真实拖动与缩放命中区仍需
  Windows WebView2 实机复验。
- M6-2 验证：`cargo test -p ff-engine` 通过，**53 项测试通过、0 失败**，其中包含
  11 项 IPC 合约测试与会话隔离回归；未开始 OS 管道或服务验收。
- 本轮连接循环验证：`cargo test -p ff-engine` 通过，**54 项测试通过、0 失败**；
  其中 IPC 合约测试 12 项。仍未开始 Windows OS 管道/ACL 实测。
- 命名管道/服务验证（#88）：workspace fmt/clippy(-D warnings)/test 全绿；新增 4 项
  SID/SDDL 单测 + 3 项真实 OS 管道端到端测试（往返、多客户端会话隔离、卷 ID 解析）通过；
  `ff-service status/bogus` SCM 命令路径实机冒烟通过。SCM 实际安装/启停与跨会话 ACL 未实测。
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
- M6-1/M6-2/M6-3 + 索引目录 + 8 项确认错误修复已提交（`91559ba`）；本轮命名管道与
  `ff-service`（ff-ipc/ff-service 新 crate + 文档）尚在工作树，未提交；
  已提交历史与当前状态以 `git log`、`git status` 为准。
