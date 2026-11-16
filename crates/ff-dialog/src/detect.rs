//! 文件对话框分类（纯逻辑，跨平台可单测）。
//!
//! 判定依据只有两项：窗口类（`GetClassNameW`）与标题（`GetWindowTextW`）。
//! Win32 标准「打开 / 另存为」对话框的窗口类是 `#32770`；标题随系统语言变化
//! （英文 `Open` / `Save As`，中文 `打开` / `另存为`）。只在标准对话框类上匹配标题，
//! 避免把普通窗口（例如标题含"打开"的应用主窗）误判为文件对话框。

/// 检测到的文件对话框类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DialogKind {
    /// 打开 / 选择文件（英文 `Open`，中文 `打开`，含 `Select File` / `浏览` 等）。
    Open,
    /// 另存为 / 保存（英文 `Save As`，中文 `另存为`）。
    SaveAs,
    /// 标准对话框类、标题未明确归类；仍允许回填（用户显式触发 + 校验编辑控件）。
    Generic,
}

/// Win32 标准对话框窗口类。
pub const DIALOG_CLASS: &str = "#32770";

/// 通用文件对话框「文件名」编辑控件 ID（`edt1`）。
/// 经典对话框与 Vista+ IFileDialog 的兼容层都保留该 ID。
pub const FILENAME_EDIT_ID: i32 = 1148;

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    let lower = haystack.to_lowercase();
    needles.iter().any(|n| lower.contains(n))
}

/// 按窗口类 + 标题判定是否为文件对话框。
///
/// 返回 `None` 表示不是标准文件对话框；否则给出分类，供 UI 提示与回填使用。
pub fn classify(title: &str, class_name: &str) -> Option<DialogKind> {
    if class_name != DIALOG_CLASS {
        return None;
    }
    let t = title.trim();
    if t.is_empty() {
        return None;
    }
    // 保存类优先：英文 "Save As" 同时含 "save"，必须先判。
    if contains_any(t, &["save as", "另存为", "保存为"]) {
        return Some(DialogKind::SaveAs);
    }
    if contains_any(t, &["save", "保存"]) {
        return Some(DialogKind::SaveAs);
    }
    if contains_any(
        t,
        &[
            "open",
            "打开",
            "select file",
            "选择文件",
            "选择文件夹",
            "select folder",
            "浏览",
            "browse",
            "导入",
            "import",
            "插入",
            "insert",
        ],
    ) {
        return Some(DialogKind::Open);
    }
    // 标准对话框类但标题不含已知关键词：保留 Generic，交用户显式触发。
    Some(DialogKind::Generic)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_english_titles() {
        assert_eq!(classify("Open", DIALOG_CLASS), Some(DialogKind::Open));
        assert_eq!(classify("Save As", DIALOG_CLASS), Some(DialogKind::SaveAs));
        assert_eq!(
            classify("Select File", DIALOG_CLASS),
            Some(DialogKind::Open)
        );
    }

    #[test]
    fn classifies_chinese_titles() {
        assert_eq!(classify("打开", DIALOG_CLASS), Some(DialogKind::Open));
        assert_eq!(classify("另存为", DIALOG_CLASS), Some(DialogKind::SaveAs));
        assert_eq!(classify("选择文件", DIALOG_CLASS), Some(DialogKind::Open));
    }

    #[test]
    fn ignores_non_dialog_window_classes() {
        // 标题含「打开」但窗口类不是 #32770 → 不是文件对话框（如浏览器主窗）。
        assert_eq!(classify("Open - Notepad", "Chrome_WidgetWin_1"), None);
        assert_eq!(classify("打开", "Notepad"), None);
    }

    #[test]
    fn generic_for_unknown_standard_dialog_title() {
        assert_eq!(
            classify("自定义标题", DIALOG_CLASS),
            Some(DialogKind::Generic)
        );
        assert_eq!(classify("", DIALOG_CLASS), None);
    }

    #[test]
    fn save_beats_open_when_both_appear() {
        // "Open or Save As" 之类组合标题按保存归类（保守）。
        assert_eq!(
            classify("Open or Save As", DIALOG_CLASS),
            Some(DialogKind::SaveAs)
        );
    }
}
