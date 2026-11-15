# Sift 交接（2026-11-15，UI 重整续接）

## 最新状态（本节覆盖下方旧轮次的下一步）

用户已认可当前 UI 调整并授权提交、进入 M6；另要求设置采用 CompressO 式底部抽屉转场。当前先提交 #83/#84 基线，再单独实现转场与 M6 工作。下方 #83 动效记录保留作历史，其旧「等 M6 确认」计划已失效。

- 已修改 `TopBar` / `ResultArea` / `StatusBar` / `Settings`、`App.tsx`、`main.tsx` 与 CSS：统一搜索框、轻筛选、显式排序、低干扰高亮、类型色图标、索引详情展开、设置控件语义及禁用、原生窗口自适应。
- 清掉 `@keyframes window-out` 后遗留的无选择器声明及多余大括号；此前它还会吞掉 `.topbar` 规则。补设置页头不收缩约束。
- 保留先前 dirty work（#83 Rust/ACL/事件/时间转换等），无后端代码改动；120ms 搜索防抖和 32px 虚拟列表未改。
- `UI_SPEC` 更新 v0.2，`DESIGN`、`DECISIONS #84` 与 `BENCH` 同步。新增 `app/scripts/check-ui.cjs`，浏览器测试工具外置，不新增依赖。
- 本轮前端构建、格式测试、浏览器交互回归通过；新设计尚未在真实 Tauri/WebView2 上重跑，旧 #83 真机证据不代表新 UI 已验收。
- 浏览器预览 `http://127.0.0.1:5177/` 使用 MockEngine，不是真实文件结果。先让用户视觉验收，再做原生真机复验；M6 等用户另行确认。
- 下一轮重点：真实 760×480 / 480×320、125% 缩放、拖动/失焦/热键/IME、状态异常与长路径；设置持久化、Shell 图标及可调列宽仍有欠账。

## 以下为上一轮 #83 历史记录

> 给下一个对话看。规范、历史、决策都在 `docs/*.md`，本文件只写「现在在哪、下一步做什么、别踩什么坑」，不重复那些内容。

## 一句话状态

M0–M5 已全部完成并提交；**当前工作树有一整轮「动效/质感收尾」改动已写完、已真机验证、门禁全绿，但还没提交**。HEAD 停在 `247629a`（docs: 裁定首屏 50ms 口径，DECISIONS #82）。

## 工作树现状（11 改 + 1 新，全部未提交）

```
 M .workbuddy/memory/2026-11-15.md     ← 工作日志（第十一轮补记，按轮次追加）
 M app/src-tauri/capabilities/default.json
 M app/src-tauri/src/main.rs           ← 窗口动效核心（HIDE_GEN / WINDOW_OUT_MS / sift:* 事件）
 M app/src/App.tsx                     ← 事件监听 + 修 Esc 隐藏
 M app/src/engine/TauriEngine.ts       ← TauriGlobal 类型加 event
 M app/src/styles.css                  ← window-in/out、--ease-spring、菜单动效、行过渡
 M docs/{BENCH,DECISIONS,DEPENDENCIES,DESIGN,UI_SPEC}.md
?? docs/evidence/gui-menu-anim.png     ← 真机菜单动效截图
```

**没有残留进程**：验证用的 `sift-app.exe` 已全部 taskkill。

## 这轮做了什么（DECISIONS #83）

按用户拍板：补规范欠账 + 轻微质感/动效，**纯 CSS + 少量 Rust 事件，不引依赖、不动色板字体**。

1. `main.rs`：`show_window`/`hide_window`/`toggle_window` 统一走 hide 流程，发 `sift:shown` / `sift:hiding` Tauri 事件；前端收事件播 `.window-enter`（140ms scale+fade 带过冲）/ `.window-exit`（90ms），退出动画播完再真隐藏。`HIDE_GEN` 代数计数防热键连按竞态。
2. `capabilities/default.json`：加 `core:event:allow-listen` / `allow-unlisten`（原有 `core:window:allow-start-dragging`）。
3. `styles.css`：`--ease-spring`、窗口进出动画、`.ctxmenu` 进入动效、`.row` 选中/悬停过渡；`prefers-reduced-motion` 下 0.01ms。
4. 顺手修真 bug：`App.tsx` 里 `invoke('hide')` 命令名错（实际注册的是 `hide_window`），错误被 `void` 吞掉，Esc 隐藏此前在真机是失效的。

### 三个坑（都已修，别再踩）

- **CSS 多写一个 `}`**：给 `.app-window` 加 `will-change` 时把块提前闭合，`display:flex`/`flex-direction`/`font-size` 被挤出选择器，布局塌成 block → 用户看到状态栏悬空、整屏错位。`will-change` 本身在 WebView2 下也会撑出滚动条，**不要加**。
- **`fetch_add` 返回旧值**：写成 `HIDE_GEN.load() == gen` 会让每次延迟隐藏 100% 被自己作废；正确是 `== gen + 1`。
- **Tauri ACL 第二次咬人**：跟 #79 的 `start_dragging` 同款——事件 `listen` 也被 ACL 拦（`plugin:event|listen not allowed by ACL`）。以后前端用任何 `window.__TAURI__.event` / `core` 新 API，先查 capabilities。

## 真机验证结论（已做，非推断）

release `sift-app.exe` + Win32 SendInput + WebView2 CDP（脚本在 `.work/gui_act.py` / `.work/cdp.py`，已 gitignore）：

- 布局恢复：`display:flex`、状态栏贴底 gap 1px、虚拟化 23 行（不是 2899）、`scrollW=760`。
- `hide_window` → `sift:hiding` → `.window-exit` → 90ms 后真隐藏（visible=false、`hide result ok=true`）。
- 连按竞态：hide 后 30ms 内 show → 500ms 后仍可见。
- Esc 空输入 → 真隐藏通过（`log: visible:false`）。
- 菜单动效 `ctx-menu-in` 命中，截图 `docs/evidence/gui-menu-anim.png`。
- 动画中间帧 `scrollW 752 / sbGap -3` 是 scale 0.98 的正常几何，不是布局损坏。

## 门禁（交接时刚跑，全绿）

根 workspace `fmt` / `clippy -D warnings` 0，`cargo test --workspace` **211 passed**；`app/src-tauri` fmt/clippy 0、`cargo test` **7 passed**、`cargo check --features custom-protocol` 0；前端 `npm run test:format` **29 assertions**、`npm run build` 50 modules（CSS 13.16 kB）。

## 下一步（接手顺序）

1. **先把这轮提交掉**，按惯例分两次（conventional commits，中文正文）：

```bash
cd /d/Sift && git add app/src-tauri/capabilities/default.json app/src-tauri/src/main.rs \
  app/src/App.tsx app/src/engine/TauriEngine.ts app/src/styles.css
git commit -m "feat(ui): 窗口唤出/隐藏动效与轻微质感收尾" -m "<正文见 DECISIONS #83>"
git add docs/ .workbuddy/memory/
git commit -m "docs(ui): 回填动效真机验证、决策 #83 与三坑记录" -m "<正文见 BENCH/DECISIONS>"
# git status --short 应为空
```

2. 提交后**停下汇报**，等用户确认是否进 M6。用户最后的话是「轻微调整后就可以 M6 阶段了」——但按 `DESIGN.md` §0 一次一个里程碑，不要自作主张开 M6。
3. 本交接文档 `docs/HANDOVER.md` 自身也未提交，可在上述文档提交里一并 `git add docs/`，让新对话删掉或留档由用户定。

## 开放项（本轮没碰，别误判为已完成）

- **M6 服务化**：`timeconv` 目前在 Tauri 外壳，M6 暴露 HTTP 时必须抽到 `ff-engine` 公共模块复用，否则复现 #81 的时间 bug。
- Top-K 部分选择未做（#71）→ 单/双字符查询 93–443 ms（#82 实测）。
- 托盘菜单、全局热键物理触发、失焦隐藏稳定性、IME 组词：未实机覆盖。
- 设置页其余项不落盘（P1-7）。

## 常用命令

```bash
export TMP=/f/Temp TMPDIR=/f/Temp TEMP=/f/Temp   # C: 盘易满，构建前必设
cd /d/Sift && cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cd /d/Sift/app && npm run test:format && npm run build
cd /d/Sift && cargo build --release --manifest-path app/src-tauri/Cargo.toml --features custom-protocol
# 真机：cd app/src-tauri/target/release && WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--remote-debugging-port=9222" ./sift-app.exe
# 验证脚本：cd /d/Sift/.work && python gui_act.py <cmd> / python cdp.py <cmd>
# 注意：全局热键被占会导致新实例 exit 101，启动前先 taskkill //IM sift-app.exe //F
```
