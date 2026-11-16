# Sift 交接（2026-11-16，M0–M6 完成，M7 第 1/2/3 项完成）

## 当前状态

- M0–M5 已完成。用户已认可当前 UI、授权提交并进入 M6。
- `e536db0`：搜索/设置 UI 重整、旧轮次窗口动效与 ACL 修复、规范及证据。
- `b9075e9`：设置轻量上下转场。用户强调风格一致，因此没有照搬 CompressO 的
  Vaul 抽屉、大圆角、拖拽把手或背景缩放；仅借鉴进入/退出节奏。
- **M6 已完成（2026-11-16，见 DECISIONS #90 / BENCH「M6 服务化与 HTTP 真机验收」）。**
  M6-1/M6-2/M6-3 + 命名管道 + `ff-service` + 普通权限 Tauri PipeClient + `ff-http`
  全部实现；SCM 生命周期、LocalSystem 跨会话 ACL 命名管道、跨进程搜索、Stop 落盘与
  重启恢复、HTTP 五条安全要求均已真机验收（管理员会话），测试后已卸载服务并清理
  http.json / 索引 / 探测程序。当前 Tauri 启动优先探测服务管道、不可用回退 InProcess。
- 新增 crate：`ff-ipc`（SID ACL 命名管道）、`ff-http`（本地 HTTP API，纯 std 手写，
  默认关闭）、`ff-service`（SCM 外壳）。
- **M7 第 1 项「属性窗口」已完成（DECISIONS #91）**：`Alt+Enter` / 右键菜单「属性」
  经 `OpenAction::Properties` 全链路调 Windows 系统文件属性对话框。真机踩坑：简单版
  `ShellExecuteW("properties")` 返回 31 被误判失败，改用 `ShellExecuteExW` +
  `SEE_MASK_INVOKEIDLIST` 后对话框真实弹出并验证成功。详见 BENCH「M7 第 1 项」。
- **M7 第 2 项「对话框集成」已完成（DECISIONS #92）**：新 crate `ff-dialog`，
  `SetWinEventHook` 检测标准「打开/另存为」对话框（`#32770` + 标题识别），
  `Alt+D` / 右键「填入打开对话框」经 UIA `ValuePattern::SetValue` 把选中文件完整路径
  写回文件名框。真机踩坑：裸 `WM_SETTEXT` 对 IFileDialog 无效（1148 是 ComboBoxEx32），
  必须 UIA；生产 API 真机读回 `MATCH=true`。详见 BENCH「M7 第 2 项」。
- **M7 第 3 项「非 NTFS 回退遍历」已完成（DECISIONS #93）**：`enumerate_volumes`
  纳入可移动卷，新增 `AutoVolumeScanner` 按文件系统分派（NTFS → MFT+USN，其余 →
  `DirWalkScanner` 目录遍历，`std::fs::read_dir` 手写、零新增依赖）；无增量的卷由引擎
  每 30s 全量重走。真机用挂载的 exFAT VHD 验收：9 条索引、CJK 名与嵌套路径正确、
  新建文件 30s 内可见（`FOUND after 30.0s`）。网络盘（`DRIVE_REMOTE`）仍不支持
  （遍历无超时/取消），已记入 DESIGN §5.4 与本文件欠账。
- **M7 剩余 3 项（内容搜索 / 硬链接多路径 / filelist 导入）
  未立项，逐项单独立项，不自动推进**。

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

## 普通权限 Tauri 接 PipeClient（2026-11-16，已实现）

- `app/src-tauri/src/main.rs`：`EngineState.backend: Arc<Backend>` 抽象两种实现：
  `InProcess`（管理员兜底）与 `Pipe`（`ff_ipc::PipeClient`，普通权限）。启动时按当前
  用户 SID 构造 `\\.\pipe\sift-<SID>` 并尝试连接，成功用 Pipe，失败回退 InProcess——
  未装服务的机器行为与 M0–M5 完全一致。六个 Tauri 命令签名与前端契约不变。
- `open` 动作双路语义：InProcess 由引擎内部执行；Pipe 由服务经 IPC 只返回 `ResolvedAction`，
  UI 进程调 `ff_engine::execute_open_checked` 以当前用户身份执行（DESIGN §12）。
- 断线退避重连：`PipeBackend::with_reconnect` 检测 `EngineError::Io` 中含 `pipe` 的错误，
  1s 退避后重建 `PipeClient` 并至多重试一次；成功发 `sift:service-status`「服务已重连」，
  失败发「服务已断开，正在重试」。前端 `App.tsx` 监听该事件走状态栏 toast。
  `status_checked` 让状态轮询也能触发重连（trait 方法为兼容契约吞掉错误）。
- 新增 `ff-engine::actions`：`execute_open` / `execute_open_checked` / `full_path`，
  把 inprocess 已有的 Win32 Shell/剪贴板 FFI 暴露给 UI 层；3 项纯函数单测。
- **已知限制**：重连后服务侧 `EngineSession` 为全新实例，旧搜索的 `request_id` 失效，
  用户需重新搜索（toast 提示已覆盖）；SCM 安装/启停、SYSTEM 跨会话 ACL 真机实测仍未做。

## 下一步顺序（M0–M6 完成，M7 第 1/2/3 项完成；以下均未立项）

1. M7 剩余 3 项逐项单独立项：内容搜索、硬链接多路径、filelist 导入。
   属性窗口（#91）、对话框集成（#92）、非 NTFS 回退遍历（#93）已分别于第 1/2/3 项完成。
2. 设置持久化、Shell 图标、列宽调整、Top-K 部分选择等既有欠账（见「保留约束与欠账」）。
3. `ff-http` 目前只实现 `GET /search`；如需 `status`/`rescan` 等端点再单独扩展并重跑安全测试。
4. UI 设置页的 HTTP 开关仍标「M6 提供 / 禁用」（UI_SPEC）；接入开关需先做设置持久化。

（上述均不自动推进；里程碑边界见 DESIGN §13，M6 证据见 BENCH / #90，M7 第 1/2/3 项见 BENCH / #91 / #92 / #93。）

## 验证

- **M7 第 3 项「非 NTFS 回退遍历」（2026-11-16）**：workspace fmt/clippy(-D warnings)/test
  全绿（**292 passed、0 failed**：ff-ntfs 37 含新增 7 项遍历单测，ff-engine 49 含新增 2 项）；
  Tauri 门禁全绿（顺带修掉上一轮遗留的 `cargo fmt` 差异）；前端 build 50 modules + 29 断言。
  真机 exFAT VHD（`diskpart` 现建现删）：`ff-cli search G: quarterly` 输出
  `文件系统 exFAT（目录遍历回退（无增量））` / `1 卷 / 9 条 / 0.003 s` /
  `G:\docs\reports\2026\quarterly-summary.txt`；CJK 与目录查询命中正确；
eports6\quarterly-summary.txt`；CJK 与目录查询命中正确；
  新建文件 `FOUND after 30.0s`（见 BENCH / #93）；VHD 与探测 example 已清理。
- **M7 第 2 项「对话框集成」（2026-11-16）**：workspace fmt/clippy(-D warnings)/test 全绿
  （**281 passed、0 failed**，ff-dialog 新增 5 项分类单测）；Tauri 门禁全绿；前端 build
  50 modules + 29 格式断言通过。真机标准「打开」对话框：`DialogWatcher` 检测到
  title=打开/kind=Open，生产 API `fill_active_dialog` 写路径后 UIA 读回 `MATCH=true`
  （裸 `WM_SETTEXT` 无效的踩坑与 UIA 方案见 BENCH / #92）；探测 example 已删、无残留。
- **M7 第 1 项「属性窗口」（2026-11-16）**：workspace fmt/clippy(-D warnings)/test 全绿
  （ff-engine 47 + IPC 契约 12，新增 Properties 线格式与 roundtrip）；Tauri 门禁全绿；
  前端 build 50 modules + 29 格式断言通过。真机 `ShellExecuteExW` + `INVOKEIDLIST`
  对 `C:\Windows\notepad.exe` 弹出系统属性对话框并验证成功（简单版 `ShellExecuteW`
  返回 31 的踩坑与修正见 BENCH / #91）；探测 example 已删、无残留窗口。
- **M6 完成轮（最终，2026-11-16）**：根 workspace fmt/clippy(-D warnings)/test 全绿，
  **275 passed、5 ignored、0 failed**（ff-http 新增 23 单测 + 9 真实 TCP 端到端，覆盖
  错误 token/Host/count/速率、无 CORS、仅回环、会话隔离）；Tauri fmt/clippy(-D warnings)/
  test/custom-protocol check 全绿；前端 build 50 modules + 29 格式断言通过。真机 SCM +
  管道 + HTTP 验收回显见 BENCH「M6 服务化与 HTTP 真机验收」。已提交见 `git log`。
- 根 workspace fmt/clippy：通过；test：**229 passed、5 ignored、0 failed**（此为 M6-1 历史批次）。
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
- 普通权限 UI 接入验证（#89）：workspace fmt/clippy(-D warnings)/test 全绿（ff-engine 46、
  ff-ipc 4+3、其余无回归）；Tauri fmt/clippy(-D warnings)/test 通过；前端 build 50 模块 +
  29 项格式断言通过。`ff-engine::actions` 新增 3 项纯函数单测；Tauri 管道模式/重连分支
  仅编译期 + ff-ipc 端到端覆盖，SCM 真机安装后需实测。
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
- 已提交三批：`91559ba`（M6-1/M6-2/M6-3 + 索引目录 + 8 项确认错误修复）、
  `286e3d3`（命名管道与 `ff-service`：ff-ipc/ff-service 新 crate + 文档）、
  本轮普通权限 UI 接入（Tauri 双后端 + 重连 + 动作执行 API，提交见 `git log`）。
  当前工作树仅有 `.workbuddy/memory/` 本地日志（不入 git）；
  已提交历史以 `git log`、当前状态以 `git status` 为准。
