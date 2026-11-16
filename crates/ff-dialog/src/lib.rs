//! Sift M7 对话框集成（DESIGN §99：`SetWinEventHook` + UI Automation，不注入）。
//!
//! 职责：监听前台窗口变化，识别标准「打开 / 另存为」文件对话框（`#32770`），
//! 记住其 HWND 快照；在用户显式触发时把选中文件的完整路径写回对话框的「文件名」控件。
//!
//! 服务模式下 UI 进程以当前用户身份运行，钩子与回填都在 UI 进程内，不经过提权服务
//! （DESIGN §12）；Sift 自身热键唤出会抢走前台，因此记忆 HWND 而非"当前前台"。
//!
//! 非 Windows 平台提供 no-op 空实现，保持 Tauri 跨平台可编译。

pub mod detect;

pub use detect::{classify, DialogKind, DIALOG_CLASS, FILENAME_EDIT_ID};

/// 最近识别到的文件对话框快照（`hwnd` 以 `usize` 承载，便于跨线程传递）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogSnapshot {
    pub hwnd: usize,
    pub title: String,
    pub kind: DialogKind,
}

/// 对话框事件：打开（成为前台）/ 关闭（已销毁）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogEvent {
    Opened(DialogSnapshot),
    Closed,
}

/// 对话框操作错误。
#[derive(Debug)]
pub enum DialogError {
    /// 钩子安装 / 线程启动失败。
    Hook(String),
    /// 当前没有已识别且仍存在的文件对话框。
    NoDialog,
    /// 对话框存在但未找到「文件名」编辑控件（非标准对话框）。
    NoEditControl,
    /// `WM_SETTEXT` 返回失败。
    SetTextFailed,
}

impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialogError::Hook(e) => write!(f, "对话框监听启动失败：{e}"),
            DialogError::NoDialog => write!(f, "当前没有检测到文件打开对话框"),
            DialogError::NoEditControl => write!(f, "对话框中没有找到文件名输入框"),
            DialogError::SetTextFailed => write!(f, "写入文件名失败"),
        }
    }
}
impl std::error::Error for DialogError {}

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::{fill_active_dialog, has_active_dialog, kind_label, DialogWatcher};

#[cfg(not(windows))]
mod stub {
    use super::*;

    /// 非 Windows 空观察器。
    pub struct DialogWatcher;
    impl DialogWatcher {
        pub fn start() -> Result<Self, DialogError> {
            Ok(Self)
        }
        pub fn take_receiver(&mut self) -> Option<std::sync::mpsc::Receiver<DialogEvent>> {
            None
        }
        pub fn active() -> Option<DialogSnapshot> {
            None
        }
    }
    pub fn fill_active_dialog(_path: &str) -> Result<(), DialogError> {
        Err(DialogError::NoDialog)
    }
    pub fn has_active_dialog() -> bool {
        false
    }
    pub fn kind_label(_kind: DialogKind) -> &'static str {
        "文件"
    }
}
#[cfg(not(windows))]
pub use stub::{fill_active_dialog, has_active_dialog, kind_label, DialogWatcher};
