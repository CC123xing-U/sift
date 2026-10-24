# Everything 1.4.1.1032 功能实现文档（重写用）

> 用途：交给 AI 重写一个功能等价的程序。只描述"功能怎么实现"，不含分析过程。
> 目标平台：Windows x64。参考实现：everything.exe（PE64，MSVC 2005，纯 C，静态 CRT，2.2MB，未加壳）。

---

## 1. 程序形态与进程模型

### 1.1 三种形态（同一二进制）

| 形态 | 触发 | 实现要点 |
|---|---|---|
| GUI 客户端 | 默认启动 | 注册窗口类（类名 `EVERYTHING`）、消息循环、托盘图标（Shell_NotifyIcon） |
| Windows 服务 | 服务控制台调用 | StartServiceCtrlDispatcherW + RegisterServiceCtrlHandlerW + ReportEventW |
| 单实例守卫 | 第二次启动 | FindWindow("EVERYTHING") 找到已有窗口 → SendMessage 激活它 → 自己退出 |

### 1.2 部署形态

- 绿色便携：所有文件与 exe 同目录
- 数据库默认与 exe 同目录，可由 ini `db_location` 改到别处
- 多实例：命令行 `-instance "%s"`，数据库名变为 `Everything.<实例名>.<??>.db`

---

## 2. 索引生成（USN Journal 引擎）

### 2.1 数据源优先级

```
1. NTFS 卷    → USN Journal（$Extend\$UsnJrnl）
2. ReFS 卷    → USN Journal（独立代码路径，与 NTFS 并列）
3. 无 journal  → 目录变更通知（FindFirstChangeNotificationW/FindNextChangeNotification）
4. 用户指定   → filelist CSV 导入（9 列，见 §9.2）、单文件夹索引
```

### 2.2 NTFS USN 操作（DeviceIoControl）

打开卷：`CreateFileW("\\\\.\\C:")` 或卷 GUID 路径 `\\?\Volume{GUID}`。

| FSCTL | 值 | 用途 |
|---|---|---|
| FSCTL_QUERY_USN_JOURNAL | 0x000900F4 | 取 journal id、first usn、maximum size |
| FSCTL_ENUM_USN_DATA | 0x000900B3 | **全量枚举**（首次建索引；输入 START=0, LowUsn=0, HighUsn=MAX） |
| FSCTL_READ_USN_JOURNAL | 0x000900BB | **增量读取**（日常监控；从上次 USN 续读） |
| FSCTL_CREATE_USN_JOURNAL | 0x000900E7 | 卷没有 journal 时创建（ini 可关） |
| FSCTL_DELETE_USN_JOURNAL | 0x000900FE3 | ini 可触发 |

枚举结束信号：`ERROR_HANDLE_EOF(38)` 是正常终止，不是错误。

权限：读 journal 需要 `SeManageVolumePrivilege`（默认禁用，需 AdjustTokenPrivileges 开启）。

### 2.3 USN_RECORD 解析（含坑）

记录头布局（V2/V3/V4 通用）：

```
+0  u32 RecordLength
+4  u16 MajorVersion   (2=V2, 3=V3/V4)
+6  u16 MinorVersion
+8  u64 FileReferenceNumber      ← 注意：V3/V4 是 128 位，只取低 48 位有效（记录号+序列号）
+16 u64 ParentFileReferenceNumber（同样取低 48 位）
+24 i64 Usn
+32 i64 TimeStamp（FILETIME）
+40 u32 Reason（位组合，见 2.4）
+44 u32 SourceInfo
+48 u32 SecurityId
+52 u32 FileAttributes
+56 u16 FileNameLength（字节数）
+58 u16 FileNameOffset（相对记录起点）
+60 文件名（UTF-16LE，无 NUL，长 FileNameLength）
```

**重大坑（本机实测）**：现代 Windows 返回的是 **V3 记录，FRN 是 128 位**，所有字段比 V2 后移 16 字节（FileAttributes@68 / FileNameLength@72 / FileNameOffset@74）。解析必须按 MajorVersion 分支。

### 2.4 reason 位 → 索引动作

| reason | 动作 |
|---|---|
| USN_REASON_FILE_CREATE | 新建 file/folder 记录 |
| USN_REASON_FILE_DELETE / CLOSE 后 delete | 标记删除（ghost，`db_clear_ghosts` 清理） |
| USN_REASON_RENAME_OLD_NAME / NEW_NAME | 改名，父指针重挂 |
| USN_REASON_DATA_EXTEND / TRUNCATION / OVERWRITE | size 列更新 |
| BASIC_INFO_CHANGE / SECURITY_CHANGE / EA_CHANGE | 属性/时间列更新 |

### 2.5 线程模型

```
主线程            UI + 查询分发
db 加载/保存线程  db_load / db_save（临界区保护）
USN 处理线程      循环：读 USN 记录 → 改内存索引（仅当该卷有 journal 才启动）
NTFS 更新线程     卷变更事件（插拔盘、journal 删除重建）
重建线程          rebuild（配置变化或 db 加载失败时）
```

节奏由 ini 控制：`monitor_pause`、`monitor_update_delay`、`monitor_retry_delay`、`monitor_thread_mode_background`、`usn_record_filter`（按 reason 位过滤）。

### 2.6 触发全量重建的条件（实现时照抄）

```
- db 文件 magic 不符 / 版本号不符 / 结构损坏
- include only 规则变化、根目录列表变化、卷 GUID 变化、卷路径变化
- ntfs/refs 索引类型变化、发现新 ntfs/refs 卷
- filelist 文件名/索引类型变化、新 filelist 卷
- folder 文件名/索引类型变化、新 folder 卷
- 索引被移除、排除列表变化、flags 变化、结构变化
- 用户手动 db_rebuild
```

版本策略：**版本不匹配直接重建，不做迁移**。

---

## 3. 内存索引结构

### 3.1 分块数组（chunked array）

folder 与 file 各一个总数组，固定 **493 条一块（0x1ED）**：

```
块: { 数据指针, 数据偏移(i32), 容量(i32)=493, 使用数(i32) }
条目: 指针数组，每项 8 字节指向 folder/file 记录
```

增长时追加新块；最后一块容量 = 剩余条数。

### 3.2 folder 记录

```
[+0] (opt, 8B) 父 folder 指针      ← 仅当 parent >= folder_count（扩展根区）时存在
[+0/8] u32  parent_index           父索引；>= folder_count 表示指向扩展根
[+8/16] u8  name_len (0xFF → 后跟 u32)
[+9/17] 名字字节（变长，增量编码，见 §4.4）
[...]   folder_properties         宽度由 flags 计算（见 3.4）
[...]   类型附加：type0 → 8 字节(u64) / type3 → 16 字节 / 其他 → 0
```

### 3.3 file 记录

同构，properties 宽度与 folder 不同。

### 3.4 属性列（按 flags 位挂载，存在才占空间）

| flags 位 | folder 列 | file 列 | 内容 |
|---|---|---|---|
| bit 8 (0x0100) | — | ✅ | date modified |
| bit 9 (0x0200) | ✅ | ✅ | date accessed |
| bit 10 (0x0400) | ✅ | ✅ | size |
| bit 11 (0x0800) | ✅ | ✅ | attributes |
| bit 12 (0x1000) | ✅ | ✅ | date created |
| bit 13 (0x2000) | ✅ | ✅ | date recently changed |
| bit 14 (0x4000) | — | ✅ | run count / date run |
| bit 0x20 且 0x100 | ✅ | — | folder 专用列 |

每列在 db 文件里是独立的 u32 索引数组（按 folder/file 序号回指记录），在内存里是指针数组。**props 是每条记录都有的基础字段；可选列在文件末尾。**

---

## 4. 数据库格式（ESDb）完整规格

### 4.1 文件布局

```
偏移   内容
0x00   u32 magic        = 0x62445345 ("ESDb" 小端)
0x04   u32 version      = 0x01070014 (17235988；不等即拒载→重建)
0x08   u32 flags        db 状态位（位表见 3.4）
0x0C   u32 folder_count
0x10   u32 file_count
0x14   varint index_count              索引/卷记录数
       卷记录 × index_count:
         varint type      0=NTFS 1=FILELIST 2=FAT 3=ReFS
         u8     number    卷编号
         type 0/3: 4×string + u64 + u64
              string: GUID路径(\\?\Volume{...}) / 挂载点("C:") / fs / label
              u64: journal id      u64: 下一个 USN
         type 1: 1×string(文件名) + u64×2
         type 2: 1×string + u64(卷序列号)
       u8    flags2        bit0/bit1 两个布尔
       3× 链表: varint count + 每项 { u8 type, string }     （run history 等，可为空）
       u32 × folder_count  父索引数组
       [folder 记录 × folder_count]
       [file   记录 × file_count]
       可选属性列（按 3.4 的 flags 位存在性）: 每列 = u32 count + u32 索引数组
```

### 4.2 读/写原语

```
read_u32 / write_u32
read_varint / write_varint      u8；值 ==0xFF 时后跟 u32（变长编码）
read_string / write_string      u8 len（0xFF → u32 len）+ 字节 + NUL
read_u8 / read_bytes / read_u64
压缩: ini db_compress=1 时记录区为 bzip2 流（64KB 缓冲，BZ2_bzDecompress）；
      头部与父索引数组保持明文
```

### 4.3 folder/file 记录（流式）

```
varint delta_len              与上一条完整名字的差量长度
if delta_len != 0:
    varint back                上一条名长度 − 公共前缀长度
    delta_len 字节             差量（UTF-8）
props 字节                     固定宽度（按 flags 计算）
类型附加: type0 → 8 字节 / type3 → 16 字节 / 其他 → 0
```

### 4.4 名字存储：前向差分（front coding）——db 小的关键

按 folder/file 数组顺序，每条只存与**上一条完整名字**的公共前缀之后的差量：

```
common     = 与上一条名的公共前缀长度
delta_len  = 本名长度 − common
back       = 上一条名长度 − common
写 varint(delta_len)；若非零再写 varint(back) + 差量字节
```

同目录相邻文件共享前缀（`a.txt` `a.bak` `a.doc`），130 万条名字平均只存十几个字节。实测：50MB db 存 130 万条。

### 4.5 保存流程（原子落盘）

```
1. 加保存锁；flush 待写队列
2. 打开写流（db_compress 决定 bzip2/明文）
3. 按 4.1 顺序写全部内容
4. flush/close
5. 目标：单实例 Everything.db / 多实例 Everything.<inst>.<??>.db
6. 先写 .tmp 再原子替换（MoveFileEx 替换语义）
7. 失败记日志 "failed to save %s %d"
```

### 4.6 加载流程

```
读 magic（不符 → "invalid db magic" → 重建）
读 version（不符 → "unsupported db version" → 重建）
读 flags/counts → 打印 "Loading structure %08x: %d folders, %d files..."
读卷记录段 → flags2 → 3 链表
读父索引数组 → 校验每个 parent < folder_count + 扩展根数
按块读 folder/file 记录：
    父链走到根 → 读差分名 → 读 props → 读类型附加 → 构造内存记录
    parent 索引换算成 folder 指针
按 flags 位读可选属性列
```

---

## 5. 搜索语法与查询执行

### 5.1 词法（帧栈机解析）

```
分词：空格分隔；引号内整体
"<" 或 "("   → 入栈（LPAREN 帧）
">" 或 ")"   → 弹栈
"!" "not:" "NOT"  → 取反帧（作用于后续 term）
AND / OR     → 优先级帧（AND/OR 谁优先可配置）
```

### 5.2 term 结构与 flags 位

```
term = { prev, next, next_term, flags(u32), sort(u32), name }
flags 位：
  0x0001 case         大小写敏感
  0x0002 wholeword    全词匹配
  0x0004 path         匹配完整路径（而非仅文件名）
  0x0008 diacritics   区分变音符号
  0x0010 file         仅文件
  0x0020 folder       仅目录
  0x0040 ascii        仅 ASCII 名
  0x0800 regex        正则
  0x1000 wholefilename 全名精确匹配
  0x4000 wildcards    通配符
  0x18400 NOT         取反（弹栈时异或 + 交换链指针）
```

### 5.3 修饰符（弹栈时把 flags 打到 term 链）

```
wholefilename:/wfn:/exact:            |= 0x1200
nowholefilename:/nowfn:/noexact:      &= ~0x100
file:/files:/nofolders:               |= 0x10, &= ~0x8000
folders:/folder:/nofiles:             |= 0x20, &= ~0x8010
nofileonly: / nofolderonly:           清 0x10 / 0x20
path: / nopath:                       |= / &= ~0x0004
case: / nocase:                       |= / &= ~0x0001
regex: / noregex:                     |= / &= ~0x0800
diacritics: / nodiacritics:           |= / &= ~0x0008
wholeword:/ww: / nowholeword:/noww:   |= / &= ~0x0002
ascii: / utf8:/noascii:               |= / &= ~0x0040
wildcards: / nowildcards:             |= / &= ~0x4100
filtercase/filterpath/filterwholeword/filterdiacritics/filterregex:（+ no 版）
                                      改的是后续 term 的默认值
```

### 5.4 属性操作符全表

```
数值/尺寸   size:>10mb  size:<=1gb    比较符 > >= < <= = / == ；单位 b/kb/mb/gb/tb/pb/eb
            fsi:                      文件大小（folder 版）
            len:                      文件名长度
日期        dc:/datecreated:  dm:/datemodified:  da:/dateaccessed:
            drc:/rc:/recentchange:    dr:/daterun:
            值支持: current/this/coming/next/past/last/prev/tomorrow/yesterday +
                     month/week/days/years/hours/mins/secs + 周日月名 + 月份名
层级        parent:  parents:  infolder:  depth:  nosubfolders:
            child:<pat>  childcount:  childfilecount:  childfoldercount:
            frn:                       文件引用号
名称        startwith:  endwith:  ext:  type:  filelistfilename:  path:
属性        attrib:/attribute:/attributes:    runcount:/rc:
内容        content:  utf8content:  utf16content:  utf16lecontent:  utf16becontent:
            contenta:/ansicontent:  contentw:/wcharcontent:/unicodecontent:
            默认内容搜索扩展表（约 40 种文本格式）:
            c;chm;cpp;csv;cxx;doc;docm;docx;dot;dotm;dotx;h;hpp;htm;html;hxx;ini;
            java;lua;mht;mhtml;odt;pdf;potx;potm;ppam;ppsm;ppsx;pps;ppt;pptm;pptx;
            rtf;sldm;sldx;thmx;txt;vsd;wpd;wps;wri;xlam;xls;xlsb;xlsm;xlsx;xltm;xltx;xml
重复项      dupe:  dmdupe:/dupedm:/datemodifieddupe:  dcdupe:/dupedc:/datecreateddupe:
            dadupe:/dupeda:/dateaccesseddupe:  sizedupe:/dupesize:
            attribdupe:/dupeattrib:/attributedupe:
媒体属性    genre: comment: album: artist: title: year: track:   （ID3）
图片属性    portrait / landscape / orientation: / dimensions: / dimension:
            bitdepth: / high: / height: / wide: / width:
其他        quot:（引号字符串）  filelist: / search-file-list:
```

### 5.5 查询执行

```
1. 解析成 term 链表（名字匹配）+ file 条件链表 + folder 条件链表
2. 名字 term：UTF-8 名字 blob 上做子串/通配符/正则/全词匹配
3. 属性条件：条件 = {property_id, flags, value_lo, value_hi}
   - size 等数值列走"预排序数组 + 二分"（ corrupt file size sorted array 错误串证明有 size 有序索引）
   - 日期列转成 u64 区间比较
4. 结果按 name(0)/path(1)/size(2)/date_modified(3) 排序（可升/降序）
5. 分页：QUERY_NEXT 游标式（offset+count）
```

---

## 6. 调用入口（四种）

### 6.1 IPC：WM_COPYDATA + `EVERYTHING <CMD>` 文本协议（最重要，优先实现）

发送方：`FindWindow("EVERYTHING")` → `SendMessage(WM_COPYDATA)`，载荷一行文本 `EVERYTHING <CMD> [参数]`。

**查询类**
```
QUERY / QUERY_NEXT / QUERY_NEXT2 / QUERY_NEXT3
QUERY_PREV / QUERY_PREV2 / QUERY_PREV3
QUERY_PAGE / QUERY_PAGE_END
QUERY_NUM / QUERY_NUM2 / QUERY_NUM3
QUERY_NUMRESULTS / QUERY_NUMRESULTS2
QUERY_CUR / QUERY_CUR2 / QUERY_NAV
```

**db 直查 API（第三方集成主力）**
```
db_query_create / db_query_search / db_cancel_query / db_destroy_query
db_get_result_count / db_get_folder_result_count / db_get_file_result_count
db_get_result_name / db_get_result_path / db_get_result_full_path_name / db_get_result_extension
db_get_result_file_attributes / db_get_result_file_info / db_get_cached_result_file_info
db_get_result_run_count / db_get_result_date_run / db_get_result_date_recently_changed
db_is_folder_result / db_is_root_result / db_is_ghost_result / db_clear_ghosts
db_select_result / db_select_result_range / db_select_all / db_invert_all_selection / db_deselect_*
db_find_first_file / db_find_next_file / db_find_nearest_string_match_result
db_get_result_index_from_filename / db_get_top_run_result_index / db_get_last_run_result_index
db_save / db_load / db_rebuild / db_refresh / db_destroy
db_request_fileinfo / db_read_extended_info_on_selection
db_monitor_pause / db_monitor_stop_by_handle|volume / db_monitor_restart_* / db_monitor_set_pause
db_filelists_is_change / db_folders_check_for_updates / db_folders_update_now|all_now
```

**窗口/视图类**
```
SEARCH / SORT / SORT_ASCENDING / SORT_DESCENDING / FILTER_* / COLUMN_* / SELECT_*
FOCUS_* / SCROLL_* / WINDOW_* / STATE_* / TOGGLE_* / SETTINGS_* / THEME_*
BOOKMARK_* / HISTORY_* / EXIT / OPEN_* / CLOSE_* / ZOOM_*
```

### 6.2 HTTP 服务端（内置 web server）

```
请求: GET /path?query HTTP/1.0
header: authorization: (Basic) / range: bytes=a-b（断点续传）
URL:   '#' 截断；'?' 后 query string；'+' → 空格；路径 '/'→'\'

query 参数:
  s|q|search     搜索词（URL 解码）
  o|offset       结果偏移
  c|n|count      结果数量（json 模式缺省不限）
  j|json         JSON 输出
  i|case         大小写敏感      w|wholeword  全词
  p|path         匹配路径        r|regex      正则      m|diacritics  变音符号
  path_column / size_column / date_modified_column   列开关
  sort           name|path|size|date_modified
  ascending      升序

响应: HTTP/1.0 200 OK\r\n + Content-Type: text/html; charset=UTF-8\r\n
      HTML 模板（DOCTYPE 4.01 + meta viewport + 链接 /main.css /favicon.ico）
      静态资源从 "HTTP Server\" 目录: index.html / main.css / favicon.ico /
      everything.png / file.png / folder.png / drive.png / icon.png
      结果分页文件名 "%d.html"
错误码: 400 Bad Request（路径不以 / 开头）/ 401 Unauthorized（设了账密而未认证）/ 404 Not Found

ini: http_server_port / http_server_bindings / http_server_username / http_server_password / allow_http_server / index_http_server
```

### 6.3 ETP 服务端（自有 TCP 协议）

- ini：`etp_server_enabled/port/bindings/username/password/welcome_message/log_file_name/logging_enabled/log_max_size/log_delta_size/allow_file_download`
- 客户端侧重写替换：`etp_client_rewrite_patterns / substitutions`
- 日志：`Logs\ETP_Server_Log.txt`
- 注：二进制帧格式未完全逆向，重写时可自定义

### 6.4 FTP 服务端

- ini：`ftp_server_*`、`ftp_links`、`ftp_check_data_connection_ip`、`ftp_allow_port`
- 支持 PASV（错误串 `500 Failed to get port address.`）
- 重写时可自定义

---

## 7. 配置系统（Everything.ini）

- 单 section `[Everything]`，键值对，**738 键**
- 保存：`Everything.ini.tmp` → 原子替换
- 分组：
  - `ntfs_volume_*` / `refs_volume_*`：`monitors / include_onlys / load_recent_changes / includes / roots / paths / guids`（双引擎成对）
  - `filelist_*` / `folder_*`：`monitor_changes / rescan_if_full_list / buffer_size_list`
  - `db_*`：`compress / location / multi_user_filename / update_thread_priority`
  - `monitor_*`：`pause / update_delay / retry_delay / thread_mode_background`
  - `search_*` / `search_history_*`：`days_to_keep / keep_forever / suggestion_max / always_suggest*`
  - `http_server_*` / `etp_server_*` / `ftp_*`
  - `window_*` / `*_column_*` / `listview_*`
  - `auto_include_fixed/removable_ntfs_volumes`、`auto_remove_offline/moved_ntfs_volumes`（refs 同套）
  - `create-file-list` / `create-file-list-include-only-files / exclude-folders / exclude-files`
  - `exclude_list_enabled`、`allow_ntfs_open_file_by_id`、`extended_information_cache_monitor`

---

## 8. UI 结构

- 主窗口 = 搜索框 + 结果列表（owner-draw ListView，双缓冲：CreateCompatibleDC/CreateDIBSection/StretchDIBits）
- 搜索即输即出（search_as_you_type）；历史建议弹层
- 托盘常驻 + 全局热键（RegisterHotKey）
- 书签 / 过滤器 / 列排序 / 主题
- 对话框资源（菜单 + 选项页），从资源段解析

---

## 9. 数据文件清单与格式

### 9.1 全部文件（与 exe 同目录）

```
Everything.exe        主程序
Everything.db(.tmp)   索引 + 原子替换临时文件
Everything.ini(.tmp)  配置
Everything.lng        语言包（8 字节头 + bzip2；解压 4.86MB；48 种语言；
                       表项 = u32 起止偏移 + u8 长度 + 名称）
Run History.csv(.tmp)     每条: Filename, Run Count, Date Run
Search History.csv(.tmp)  搜索历史
Bookmarks.csv(.tmp)       书签
Filters.csv(.tmp)         过滤器
Errorlog.txt              错误日志
Logs\ETP_Server_Log.txt / Logs\HTTP_Server_Log.txt
HTTP Server\*.png/*.css/*.ico
Everything.chm / License.txt / Changes.txt / Uninstall.exe
Settings.c / Merge.c / Merge.ini / Merge2.ini   （配置导出/多机合并）
```

### 9.2 filelist CSV 导入格式（9 列，列名大小写不敏感、列序任意）

```
Filename/Name, Size, Date Modified, Date Created, Date Accessed, Attributes,
Run Count, Date Run, Recently Changed
```

---

## 10. 重写要点（最小充分集 + 坑）

### 10.1 最小充分集

1. USN 读取（QUERY/ENUM/READ + 提权）
2. 内存分块数组 + 属性列
3. ESDb 读写（含差分与 bzip2）——或自定义格式
4. 查询解析（帧栈机 + 修饰符 + 属性操作符）
5. 一种调用入口（文本 IPC 或 HTTP）
6. UI（ListView + 托盘 + 热键）

### 10.2 坑（实测）

1. **USN_RECORD_V3 的 FRN 是 128 位**，V2 之后字段整体后移 16 字节
2. `FSCTL_QUERY_USN_JOURNAL = 0x000900F4`（METHOD_BUFFERED，函数号 61——别按 NEITHER 推成 60）
3. `ERROR_HANDLE_EOF(38)` 是枚举正常结束
4. `HighUsn` 必须填 journal 的 NextUsn（不是 0x7FFF...）
5. 需要 AdjustTokenPrivileges 启用 `SeManageVolumePrivilege`（+`SeBackupPrivilege`）
6. 父索引数组里 `>= folder_count` 的值表示扩展根区（根目录列表）
7. db 版本不匹配直接重建，不要试图迁移

### 10.3 性能参考（重写目标）

- 172 万条（3 卷）建索引 ~70s；落盘 84MB/4.4s；载入 0.26s
- 子串搜索 26–182ms；通配 `*.dll` ~94ms
- 手段：整名字 blob + `bytes.find`（C 级子串）+ bisect 回映射；数值列预排序 + 二分；懒建倒排
