# ff-ntfs M0 实测与 ntfs-reader 发现

日期：2026-10-24。范围含 MFT 全量扫描和 M0 朴素子串搜索。

> 口径更正（第三轮文档复验）：USN 增量**已实现**——`NtfsVolumeScanner::read_changes`（`src/scanner.rs`）走
> `FSCTL_READ_USN_JOURNAL` + `usn` 模块解析，不再返回 `ScanError::Unsupported`（见 `docs/DECISIONS.md` 决策 25）。
> 本文件保留的是 M0 当时的摸底口径；**USN 增量尚未被 `ff-engine` / `ff-cli` 接线调用**，M2 的
> Base+Overlay、后台合并与持久化（DESIGN §6）仍未落地。

## ntfs-reader 版本与接口

-  crate：`ntfs-reader 0.6.0`，`MIT OR Apache-2.0`，MSRV 1.88.0。
-  本实现使用 `Volume::new`、`MftScan::new` / `next_chunk`，按 4096 条记录（默认 4 MiB 窗口）分块读取，不把整个 `$MFT` 驻留内存。
- `MftScan` 实际会顺序读取 `$MFT` 两遍：第一遍建立扩展记录和目录名索引，第二遍逐块产出文件。活卷不是快照，两遍之间发生变化时可能看到不一致的目录名或属性。

## 可提供字段及本实现映射

| RawEntry 字段 | ntfs-reader 0.6.0 来源 | 结论 |
|---|---|---|
| `frn` | `NtfsFile::reference()` | 可提供；高 16 位为序列号，作键时屏蔽 |
| `parent_frn` | `best_name().parent_reference()` | 可提供 |
| `name` | `best_name()` | 优先 Win32 名，否则第一个可用名 |
| `is_dir` | `is_directory()` | 可提供 |
| `attrs` | `standard_information().file_attributes()` | 可提供；缺失时本实现写 0 |
| `size` | 未命名 `$DATA` 属性的 `value_size()` | 可提供默认流逻辑大小；缺失时写 0，不含 ADS 大小 |
| `mtime` | `standard_information().modified()` | 可提供 |
| `ctime` | `standard_information().created()` | 可提供；这里是 NTFS 创建时间 |
| `atime` | `standard_information().accessed()` | 可提供 |

时间由 `time::OffsetDateTime` 转为契约统一的 FILETIME（1601 纪元、100 ns）。扫描只使用 `chunk.files()` 的在用记录；`files()` 不包含 NTFS 保留的前 24 条记录，因此根目录记录 5 不会作为普通条目产出，后续索引构建需合成卷根。

## 权限与操作要求

- 必须以管理员权限打开原始卷，例如 `\\.\C:`；盘符设备路径不能带尾随反斜杠。GUID 路径在 `GetVolumeInformationW` 时保留尾随反斜杠，交给原始卷读取前移除。
- `full_scan` 在读取 MFT 前调用 `FSCTL_QUERY_USN_JOURNAL` 并保存 `journal_id` / `next_usn`；查询失败时尝试创建后再查，仍失败则返回 `(0, 0)`，由 CLI 提示退化为仅全量重扫。
- 直接读卷可能触发安全软件关注；当前实现只读 MFT，除 USN Journal 不存在时按设计尝试创建外，不写卷内容。

## 限制与坑

- **硬链接**：crate 的 `hard_links()` 能枚举多个名字；M0 为每个 MFT 基记录只取 `best_name()`，因此当前索引只保留一个名字，多路径支持留待 M7。
- **活卷一致性**：M0 没有消费扫描期间积压的 USN 变化，不能把结果视为严格快照。M2 必须从扫描前记录的 `next_usn` 续读；日志截断或 ID 改变必须重建。
- **名字有损边界**：crate 可经 `to_os_string()` 保留未配对 UTF-16 代理项；冻结契约使用 `String`，当前 `to_string()` 会以 U+FFFD 替代无效 UTF-16。
- **元数据缺失**：损坏或特殊记录可能没有最佳名字、标准信息或默认数据流；无名记录跳过，其余缺失字段写 0。父 FRN 无法在当前条目集合解析时计入 `warnings`。
- **性能/内存口径**：`MftScan` 降低 MFT 本体内存但需两遍 I/O；本 M0 CLI 还保留 `Vec<RawEntry>`，扫描器保留 FRN/父计数集合，所以下述峰值不是未来 SoA 索引的内存指标。
- **搜索口径**：朴素搜索逐条执行 `name.to_lowercase().contains(needle)`，包含每条名字的 Unicode 小写转换和临时分配；仅在计时前转换查询串。它不是后续 memchr/rayon 快速路径。

## M0 真实 C: 数据

- 机器：Windows 10 专业工作站版 x64；AMD Ryzen 5 5600U（6 核 12 线程）；15.3 GB 内存；C: 为固定 NTFS 卷。
- 构建/运行：release，`ff-cli scan C: ntdll`，管理员权限，单次实测。
- 卷序列号：`0x9EA0C151`。
- 条目数：**849,366**。
- 全量耗时：**5.312 s**（含扫描前 USN 查询、MFT 两遍读取及写入 `Vec<RawEntry>`，不含卷枚举和搜索）。
- 父解析警告：**0**。
- 扫描后峰值 WorkingSet：**149,585,920 B（142.66 MiB）**；当前 WorkingSet：112,054,272 B（106.86 MiB）。
- 朴素子串搜索：`ntdll`，命中 **60**，耗时 **88.022 ms**。
- 扫描前 USN：Journal ID `133932538930439016`，Next USN `24804370080`。

该数据为一次实机摸底，不是多次平均或稳定性能承诺；工作区构建位于 D:，被扫描卷为 C:。
