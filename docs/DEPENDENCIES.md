# DEPENDENCIES.md —— 依赖登记簿（Orchestrator 维护）

> DESIGN §0.2：引入任何 crate 前先读 docs.rs / 源码确认版本与接口，并登记于此。
> 不得引入 DESIGN §2.1 清单之外的重量级依赖；确需引入先向用户提出理由。
> 许可证必须核对登记（DESIGN §15：nucleo 系列需特别留意）。

| 名称 | 版本 | 许可证 | 用途 | 引入 crate | 登记日 |
|---|---|---|---|---|---|
| ntfs-reader | 0.6.0 | MIT OR Apache-2.0 | MFT 分块读取（§5，M0 全量扫描） | ff-ntfs | 2026-10-24 |
| windows | 0.62.2 | MIT OR Apache-2.0 | Win32 封装：卷枚举 / USN Journal / `GetProcessMemoryInfo` / 设备路径 | ff-ntfs、sift-bench（memprobe 进程内存） | 2026-10-24（第三轮补登 sift-bench） |
| rayon | 1.12.0 | MIT OR Apache-2.0 | 名字 arena 并行扫描（§7 子串/AND）；AND 候选并行校验 | ff-core、ff-match | 2026-10-24（第三轮补登 ff-match） |
| memchr | 2.8.3 | Unlicense OR MIT | `memmem` 子串快速路径（替代逐条 `contains`） | ff-match（经 ff-core 的 ff-match 依赖间接使用） | 2026-10-24（第三轮更正引入 crate） |
| rustc-hash | 2.1.3 | MIT OR Apache-2.0 | `FxHasher` 名称→id 反查索引（避免标准 HashMap 开销） | ff-core、ff-ntfs（FRN 集合 / 父计数） | 2026-10-24（第三轮补登 ff-ntfs） |
| criterion | 0.8.2 | Apache-2.0 OR MIT | 合成数据 criterion 基准（§14，dev-dependency） | sift-bench | 2026-10-24 |
| time | 0.3.55 | MIT OR Apache-2.0 | `ntfs-reader` 的 `OffsetDateTime` → FILETIME 转换（§4.1/§5.1 时间单位） | ff-ntfs | 2026-10-24 |
| tracing | 0.1.44 | MIT | USN 跳过的 V3/未知版本记录与解析中断的日志（§5.3；§2.1 准入） | ff-ntfs | 2026-10-24 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | M6 长度前缀 JSON 协议编解码（§10.1），从已有锁定版本复用 | ff-engine（新增直接依赖，Tauri 已有） | 2026-11-15 |
| windows-service | 0.8.1 | MIT OR Apache-2.0 | M6 Windows 服务外壳 / SCM 安装卸载（§2.1 技术栈已列、§12）。依赖 windows-sys 0.61（本地已有 0.61.2，无冲突），MSRV 1.71 | ff-service | 2026-11-16 |
| windows（新增 feature） | 0.62.2 | MIT OR Apache-2.0 | M6 命名管道：`Win32_System_Pipes` / `Win32_Security` / `Win32_Security_Authorization`（SID+SDDL ACL）、`Win32_Storage_FileSystem`（ReadFile/WriteFile） | ff-ipc | 2026-11-16 |

> 说明：`windows` 会连带拉入 `windows-sys` / `windows-numerics` 等子 crate（均为 MIT OR Apache-2.0）；`ff-core`/`ff-match` 保持 `#![forbid(unsafe_code)]`，仅依赖纯逻辑 crate；`ntfs-reader`/`windows` 经 `#[cfg(windows)]` 隔离，非 Windows 平台下 `ff-ntfs` 仍编译通过但不含实扫逻辑。
>
> `tracing` 只作为**库依赖**引入（DESIGN §2.1 已列入准入技术栈）：`ff-ntfs` 不引入订阅者（`tracing-subscriber`），后者归上层 `app` / `ff-service`（M3/M6）负责，避免库替应用决定日志输出方式。
>
> **M5（2026-11-15）：本轮零新增依赖。** T2 拼音用 `ff-match/src/pinyin.rs`
> 内置精简汉字字典，T3 模糊用 `matcher.rs` 的有序子序列匹配，均在现有
> `memchr` / `rayon` 之上实现，零 `unsafe`、零平台依赖。DESIGN §8.1 点名的
> `ib-pinyin` / `nucleo-matcher` 是生产库替换选项，引入前须按 §2.1 核对版本
> 与接口、核对许可证（§15 特别提醒 nucleo 系列条款）、在本表登记，并回归
> 金标准用例与 BENCH 实测。理由与触发条件见 `docs/DECISIONS.md` #70。
>
> **M5 收尾轮（2026-11-15）：同样零新增依赖。** P0-2 复制路径用
> `ff-engine/src/inprocess.rs` 内 `#[cfg(windows)]` 的 `user32` / `kernel32` FFI
> 直接写 `CF_UNICODETEXT`（与既有 `shell_open` 的 `shell32` FFI 同风格），
> 未引入 `arboard` 或 Tauri clipboard 插件；P0-3 右键菜单与 P1-1 拖拽区
> 均为纯 React / CSS / Tauri 内建 `data-tauri-drag-region`，无新包。
> 详见 `docs/DECISIONS.md` #74–#76。
>
> **P1-8 收尾轮（2026-11-15）：同样零新增依赖。** FILETIME → JS epoch 毫秒
> 转换是 `app/src-tauri/src/timeconv.rs` 内的纯 `i128` 算术，无新 crate；
> 前端回归断言 `app/scripts/check-format.mjs` 复用已有 devDependency
> `typescript`（`transpileModule` + `data:` URL 动态导入），未引入
> vitest / jest，也未直接依赖 vite 的传递依赖 esbuild。目录大小与时间无效值
> 兜底均为既有 React / CSS / TS 逻辑。详见 `docs/DECISIONS.md` #81。
>
> **动效与质感收尾轮（2026-11-15）：同样零新增依赖。** 窗口唤出/隐藏与右键菜单
> 动效全部为 CSS keyframes + `cubic-bezier` 曲线（`--ease-spring` 手感参考
> CompressO 的 Framer Motion spring 与 ZTools 的移动端交互，但**未引入**
> framer-motion 或任何动画库）；Rust 侧延迟隐藏用标准库线程 + `AtomicU64`
> 代数计数，无新 crate。详见 `docs/DECISIONS.md` #83。
>
> **UI 重整（#84）：零新增应用/构建依赖。** 复用 React、CSS、既有 SVG 与原生
> select/details。`test:ui` 使用外部安装的 Playwright/Chromium，通过环境变量定位；
> 不进入 package.json 依赖或锁文件，不打进安装包，复现见 `BENCH.md`。
>
> **M6 本地 HTTP（#90）：零新增依赖。** `ff-http` 用纯 std 手写 HTTP/1.1，刻意不引入
> DESIGN §2.1 候选的 axum/tokio/hyper（单一只读端点、C: 空间紧张、保持可精确单测）；
> 配置文件解析复用 ff-http 已有的 `serde_json 1.0.154`（已登记），token 生成用
> std 时间/原子计数 + xorshift，无 `getrandom`/`rand`。`ff-service` 只新增一个内部
> crate 依赖。偏离选型理由见 `DECISIONS #90`。

> **M7 对话框集成（#92）：无新增 crate，但启用了既有 `windows` crate 的 UIA features。**
> 新 crate `ff-dialog` 依赖 `windows 0.62`（workspace 已由 ff-ipc/ff-ntfs 引入，非新 crate），
> 新增 feature：`Win32_System_Com` / `Win32_System_Variant` / `Win32_System_Ole` /
> `Win32_UI_Accessibility` / `UI_UIAutomation`（均为 Windows SDK 绑定，不引入第三方运行时）。
> 事件监听（SetWinEventHook）用裸 user32 extern，无 winapi 等额外依赖；UIA COM 经
> windows crate 自动生成的绑定调用，未手写 vtable。偏离"零依赖"传统的原因（Vista+
> IFileDialog 文件名框必须走 UIA ValuePattern）见 `DECISIONS #92`。

> **M7 非 NTFS 回退遍历（#93）：零新增依赖。** DESIGN §5.4 原本写「`walkdir` 或
> `ignore`」，实际用 `std::fs::read_dir` 手写递归（`crates/ff-ntfs/src/walk.rs`），
> 不引入任何新 crate：需要的能力只有「递归 + 不跟随重解析点 + 复用排除集」，标准库够用。
> 新文件系统分派（`auto.rs`）只调既有的 `windows 0.62`（`GetVolumeInformationW`），
> 未新增 feature。**`Cargo.toml` / `Cargo.lock` 的依赖集合本轮未变**（可用
> `git diff Cargo.lock` 复核：无新增 package）。
