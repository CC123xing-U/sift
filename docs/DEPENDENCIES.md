# DEPENDENCIES.md —— 依赖登记簿（Orchestrator 维护）

> DESIGN §0.2：引入任何 crate 前先读 docs.rs / 源码确认版本与接口，并登记于此。
> 不得引入 DESIGN §2.1 清单之外的重量级依赖；确需引入先向用户提出理由。
> 许可证必须核对登记（DESIGN §15：nucleo 系列需特别留意）。

| 名称 | 版本 | 许可证 | 用途 | 引入 crate | 登记日 |
|---|---|---|---|---|---|
| ntfs-reader | 0.6.0 | MIT OR Apache-2.0 | MFT 分块读取（§5，M0 全量扫描） | ff-ntfs | 2026-10-24 |
| windows | 0.62.2 | MIT OR Apache-2.0 | Win32 封装：卷枚举 / USN Journal / `GetProcessMemoryInfo` / 设备路径 | ff-ntfs | 2026-10-24 |
| rayon | 1.12.0 | MIT OR Apache-2.0 | 名字 arena 并行扫描（§7 子串/AND） | ff-core | 2026-10-24 |
| memchr | 2.8.3 | Unlicense OR MIT | `memmem` 子串快速路径（替代逐条 `contains`） | ff-core | 2026-10-24 |
| rustc-hash | 2.1.3 | MIT OR Apache-2.0 | `FxHasher` 名称→id 反查索引（避免标准 HashMap 开销） | ff-core | 2026-10-24 |
| criterion | 0.8.2 | Apache-2.0 OR MIT | 合成数据 criterion 基准（§14，dev-dependency） | sift-bench | 2026-10-24 |
| time | 0.3.55 | MIT OR Apache-2.0 | `ntfs-reader` 的 `OffsetDateTime` → FILETIME 转换（§4.1/§5.1 时间单位） | ff-ntfs | 2026-10-24 |
| tracing | 0.1.44 | MIT | USN 跳过的 V3/未知版本记录与解析中断的日志（§5.3；§2.1 准入） | ff-ntfs | 2026-10-24 |

> 说明：`windows` 会连带拉入 `windows-sys` / `windows-numerics` 等子 crate（均为 MIT OR Apache-2.0）；`ff-core`/`ff-match` 保持 `#![forbid(unsafe_code)]`，仅依赖纯逻辑 crate；`ntfs-reader`/`windows` 经 `#[cfg(windows)]` 隔离，非 Windows 平台下 `ff-ntfs` 仍编译通过但不含实扫逻辑。
>
> `tracing` 只作为**库依赖**引入（DESIGN §2.1 已列入准入技术栈）：`ff-ntfs` 不引入订阅者（`tracing-subscriber`），后者归上层 `app` / `ff-service`（M3/M6）负责，避免库替应用决定日志输出方式。
