//! UI 进程侧动作执行（DESIGN §12）。
//!
//! 服务模式下，`Open` 经 IPC 只拿到 [`ResolvedAction`]（结果行 + 动作），服务绝不以
//! SYSTEM 身份打开文件/写剪贴板；真实动作必须由 UI 进程以当前用户身份执行。
//! 本模块把 inprocess 内已有的 Win32 FFI（`shell_open` / `clipboard_set_text`）暴露为
//! 公开入口，供 Tauri 命令层在拿到 `last_open()` 后调用。
//!
//! InProcess 模式仍走 `InProcessEngine::open`（内部已执行动作），不经过这里。

use crate::api::{EngineError, OpenAction, Result, ResultRow};

/// 从结果行拼出完整路径（`dir` 含末尾 `\` 时不再重复添加）。
///
/// 与 `MockEngine` 的 `e.dir + (endsWith('\\') ? '' : '\\') + e.name` 一致。
pub fn full_path(row: &ResultRow) -> String {
    if row.dir.ends_with('\\') || row.dir.ends_with('/') {
        format!("{}{}", row.dir, row.name)
    } else {
        format!("{}\\{}", row.dir, row.name)
    }
}

/// 在 **UI 进程**以当前用户身份执行一个已解析的打开动作。
///
/// - `CopyPath` / `CopyName`：写系统剪贴板；
/// - `Open` / `OpenFolder`：`ShellExecuteW`（Windows）/ no-op（非 Windows，见 inprocess）。
///
/// 失败返回 `Err`，由前端 `onOpenError` 显示提示（P1-9）。
pub fn execute_open(action: OpenAction, row: &ResultRow) -> Result<()> {
    let full = full_path(row);
    match action {
        OpenAction::CopyPath => super::inprocess::clipboard_set_text(&full),
        OpenAction::CopyName => super::inprocess::clipboard_set_text(&row.name),
        OpenAction::Open => super::inprocess::shell_open(&full, false),
        OpenAction::OpenFolder => super::inprocess::shell_open(&full, true),
    }
}

/// 完整路径缺失（空 `dir` 且空 `name`）时给出明确错误，避免静默打开空路径。
pub fn execute_open_checked(action: OpenAction, row: &ResultRow) -> Result<()> {
    if row.name.is_empty() {
        return Err(EngineError::NotFound);
    }
    execute_open(action, row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(dir: &str, name: &str) -> ResultRow {
        ResultRow {
            name: name.into(),
            dir: dir.into(),
            is_dir: false,
            size: Some(1),
            mtime: None,
            name_matches: Vec::new(),
        }
    }

    #[test]
    fn full_path_appends_separator_when_missing() {
        assert_eq!(full_path(&row(r"D:\Work", "a.txt")), r"D:\Work\a.txt");
    }

    #[test]
    fn full_path_keeps_existing_separator() {
        assert_eq!(full_path(&row("C:\\", "a.txt")), "C:\\a.txt");
        assert_eq!(full_path(&row(r"D:\Work\", "b.txt")), r"D:\Work\b.txt");
    }

    #[test]
    fn execute_open_checked_rejects_empty_name() {
        assert!(execute_open_checked(OpenAction::Open, &row("", "")).is_err());
    }
}
