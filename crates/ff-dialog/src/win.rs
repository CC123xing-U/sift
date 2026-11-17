//! Windows 实现：`SetWinEventHook` 监听前台窗口变化 + UI Automation `ValuePattern` 回填。
//!
//! 不注入任何 DLL（DESIGN §99）：事件钩子用 `WINEVENT_OUTOFCONTEXT`，回调投递到本进程
//! 专用线程的消息循环；回填通过 UIA 找到「文件名(N):」编辑框（AutomationId=`1148`），
//! 用 `IUIAutomationValuePattern::SetValue` 写入——Vista+ IFileDialog 用 DirectUI，
//! 经典 `WM_SETTEXT` 对文件名框无效（真机实测写入读回为空），必须用 ValuePattern。
//! 记住的是对话框 HWND 快照，而非"当前前台"——因为 Sift 自身唤出后会抢走前台，
//! 回填时仍要写回用户先前打开的那个对话框。

use std::ffi::c_void;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use super::{classify, DialogError, DialogEvent, DialogKind, DialogSnapshot, DIALOG_CLASS};

// Win32 句柄类型名沿用 SDK 大写缩写（HWND / HWINEVENTHOOK），便于与文档对照。
#[allow(clippy::upper_case_acronyms)]
type HWND = *mut c_void;
#[allow(clippy::upper_case_acronyms)]
type HWINEVENTHOOK = *mut c_void;
#[allow(clippy::upper_case_acronyms)]
type WINEVENTPROC = Option<unsafe extern "system" fn(HWINEVENTHOOK, u32, HWND, i32, i32, u32, u32)>;

const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
const OBJID_WINDOW: i32 = 0;
/// 0 = WINEVENT_OUTOFCONTEXT：回调经消息队列投递到安装钩子的线程，无需注入目标进程。
const WINEVENT_OUTOFCONTEXT: u32 = 0;
const WM_QUIT: u32 = 0x0012;

#[link(name = "user32")]
extern "system" {
    fn SetWinEventHook(
        event_min: u32,
        event_max: u32,
        hmod: *mut c_void,
        proc: WINEVENTPROC,
        id_process: u32,
        id_thread: u32,
        flags: u32,
    ) -> HWINEVENTHOOK;
    fn UnhookWinEvent(hook: HWINEVENTHOOK) -> i32;
    fn GetMessageW(msg: *mut Msg, hwnd: HWND, min: u32, max: u32) -> i32;
    fn TranslateMessage(msg: *const Msg) -> i32;
    fn DispatchMessageW(msg: *const Msg) -> isize;
    fn PostThreadMessageW(id_thread: u32, msg: u32, wparam: usize, lparam: isize) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetWindowTextW(hwnd: HWND, buf: *mut u16, max: i32) -> i32;
    fn GetClassNameW(hwnd: HWND, buf: *mut u16, max: i32) -> i32;
    fn IsWindow(hwnd: HWND) -> i32;
    fn SetForegroundWindow(hwnd: HWND) -> i32;
}

#[repr(C)]
struct Msg {
    hwnd: HWND,
    message: u32,
    wparam: usize,
    lparam: isize,
    time: u32,
    pt_x: i32,
    pt_y: i32,
}

/// 最近一次识别到的文件对话框（回调线程写，命令线程读）。
static G_CURRENT: OnceLock<Mutex<Option<DialogSnapshot>>> = OnceLock::new();
static G_TX: OnceLock<Mutex<Option<Sender<DialogEvent>>>> = OnceLock::new();

fn current_state() -> &'static Mutex<Option<DialogSnapshot>> {
    G_CURRENT.get_or_init(|| Mutex::new(None))
}
fn event_sender() -> &'static Mutex<Option<Sender<DialogEvent>>> {
    G_TX.get_or_init(|| Mutex::new(None))
}

unsafe fn read_text(hwnd: HWND, class: bool) -> String {
    let mut buf = [0u16; 512];
    let n = if class {
        GetClassNameW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
    } else {
        GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32)
    };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

unsafe fn class_of(hwnd: HWND) -> String {
    read_text(hwnd, true)
}

/// 前景窗口事件回调（运行在钩子线程）。
unsafe extern "system" fn on_foreground(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    if event != EVENT_SYSTEM_FOREGROUND || id_object != OBJID_WINDOW || hwnd.is_null() {
        return;
    }
    let class = class_of(hwnd);
    let title = read_text(hwnd, false);
    let mut guard = current_state().lock().unwrap();
    if let Some(kind) = classify(&title, &class) {
        let snap = DialogSnapshot {
            hwnd: hwnd as usize,
            title,
            kind,
        };
        *guard = Some(snap.clone());
        if let Some(tx) = event_sender().lock().unwrap().as_ref() {
            let _ = tx.send(DialogEvent::Opened(snap));
        }
    } else if guard
        .as_ref()
        .map(|s| IsWindow(s.hwnd as HWND) == 0)
        .unwrap_or(false)
    {
        // 前景已不是文件对话框，且记住的对话框已被销毁 → 清除并通知。
        // 注意：Sift 自身唤出也会让前景变成非对话框，但此时对话框仍存在（IsWindow≠0），
        // 不清除，保证回填仍能写回用户先前打开的那个对话框。
        *guard = None;
        if let Some(tx) = event_sender().lock().unwrap().as_ref() {
            let _ = tx.send(DialogEvent::Closed);
        }
    }
}

/// 钩子线程：安装 OUTOFCONTEXT 钩子后跑消息循环，回调由系统投递到本线程队列。
/// 把 `SetWinEventHook` 的结果随线程 ID 一起回传：`None` 表示钩子安装失败，
/// 让 `start()` 能据此返回 Err（此前无条件先发线程 ID，失败分支永远不可达，
/// 钩子静默失效而调用方误以为监听已建立）。
fn hook_thread(id_tx: Sender<Option<u32>>) {
    unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            std::ptr::null_mut(),
            Some(on_foreground),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
        if hook.is_null() {
            let _ = id_tx.send(None);
            return;
        }
        let _ = id_tx.send(Some(GetCurrentThreadId()));
        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        UnhookWinEvent(hook);
    }
}

/// 文件对话框观察器。持有钩子线程；`Drop` 时投递 `WM_QUIT` 并回收线程。
pub struct DialogWatcher {
    thread_id: u32,
    thread: Option<std::thread::JoinHandle<()>>,
    rx: Option<Receiver<DialogEvent>>,
}

impl DialogWatcher {
    /// 启动监听。单进程只应有一个实例（全局快照槽）；重复启动会替换事件接收端。
    pub fn start() -> Result<Self, DialogError> {
        let (tx, rx) = mpsc::channel();
        *event_sender().lock().unwrap() = Some(tx);
        *current_state().lock().unwrap() = None;
        let (id_tx, id_rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("sift-dialog-hook".into())
            .spawn(move || hook_thread(id_tx))
            .map_err(|e| DialogError::Hook(e.to_string()))?;
        let thread_id = id_rx.recv().map_err(|e| DialogError::Hook(e.to_string()))?;
        let thread_id = thread_id.ok_or_else(|| {
            DialogError::Hook("SetWinEventHook 返回空句柄（对话框监听未建立）".into())
        })?;
        Ok(Self {
            thread_id,
            thread: Some(thread),
            rx: Some(rx),
        })
    }

    /// 取出对话框事件接收端（只能取一次，交给 Tauri 事件转发线程）。
    pub fn take_receiver(&mut self) -> Option<Receiver<DialogEvent>> {
        self.rx.take()
    }

    /// 读取当前记住的对话框快照（命令线程调用）。
    pub fn active() -> Option<DialogSnapshot> {
        current_state().lock().unwrap().clone()
    }
}

impl Drop for DialogWatcher {
    fn drop(&mut self) {
        unsafe {
            PostThreadMessageW(self.thread_id, WM_QUIT, 0, 0);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        *event_sender().lock().unwrap() = None;
        *current_state().lock().unwrap() = None;
    }
}

/// 用 UI Automation 把路径写入对话框的「文件名(N):」编辑框。
///
/// 查找策略：`ElementFromHandle(dialog)` → 后代中所有 `Edit` control type →
/// 优先 `AutomationId == "1148"` 或名字含「文件名 / File name」者 →
/// `GetCurrentPattern(UIA_ValuePatternId)` → `SetValue(BSTR(path))`。
/// 只对找到的**一个**目标写入，避免误改文件列表里的其它 Edit（列表项 id 是
/// `System.ItemNameDisplay` 等，不会命中 1148 / 文件名）。
unsafe fn uia_set_filename(dialog: HWND, path: &str) -> Result<(), DialogError> {
    use std::mem::ManuallyDrop;
    use windows::core::{Interface, BSTR};
    use windows::Win32::Foundation::HWND as WinHwnd;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::System::Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_I4};
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationValuePattern, TreeScope_Descendants,
        UIA_ControlTypePropertyId, UIA_EditControlTypeId, UIA_ValuePatternId,
    };

    // COM 初始化：命令线程可能是首次调用；S_FALSE（已初始化）也视为成功。
    let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    if hr.is_err() {
        return Err(DialogError::Hook(format!("COM 初始化失败: {hr:?}")));
    }
    let automation: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL)
        .map_err(|e| DialogError::Hook(e.to_string()))?;
    let root = automation
        .ElementFromHandle(WinHwnd(dialog))
        .map_err(|_| DialogError::NoEditControl)?;

    let var = VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt: VT_I4,
                Anonymous: VARIANT_0_0_0 {
                    lVal: UIA_EditControlTypeId.0,
                },
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
            }),
        },
    };
    let cond = automation
        .CreatePropertyCondition(UIA_ControlTypePropertyId, &var)
        .map_err(|_| DialogError::NoEditControl)?;
    let edits = root
        .FindAll(TreeScope_Descendants, &cond)
        .map_err(|_| DialogError::NoEditControl)?;
    let count = edits.Length().map_err(|_| DialogError::NoEditControl)?;

    for i in 0..count {
        let el = match edits.GetElement(i) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let id = el.CurrentAutomationId().unwrap_or_default().to_string();
        let name = el.CurrentName().unwrap_or_default().to_string();
        let is_filename =
            id == "1148" || name.contains("文件名") || name.to_lowercase().contains("file name");
        if !is_filename {
            continue;
        }
        let pat = el
            .GetCurrentPattern(UIA_ValuePatternId)
            .map_err(|_| DialogError::NoEditControl)?;
        let vp: IUIAutomationValuePattern = pat.cast().map_err(|_| DialogError::NoEditControl)?;
        vp.SetValue(&BSTR::from(path))
            .map_err(|_| DialogError::SetTextFailed)?;
        return Ok(());
    }
    Err(DialogError::NoEditControl)
}

/// 把完整路径写入最近识别到的文件对话框的「文件名」控件。
///
/// 仅当对话框仍存在且类名为 `#32770` 时才写入；失败返回明确错误供 UI 提示。
pub fn fill_active_dialog(path: &str) -> Result<(), DialogError> {
    let snap = DialogWatcher::active().ok_or(DialogError::NoDialog)?;
    let hwnd = snap.hwnd as HWND;
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 || class_of(hwnd) != DIALOG_CLASS {
            *current_state().lock().unwrap() = None;
            return Err(DialogError::NoDialog);
        }
        uia_set_filename(hwnd, path)?;
        SetForegroundWindow(hwnd);
    }
    Ok(())
}

/// 供 Tauri 命令判断当前是否有可回填的对话框（不改变状态）。
pub fn has_active_dialog() -> bool {
    DialogWatcher::active().is_some()
}

/// 供 UI 提示使用的分类文案（与 `DialogKind` 解耦，避免前端依赖枚举名）。
pub fn kind_label(kind: DialogKind) -> &'static str {
    match kind {
        DialogKind::Open => "打开",
        DialogKind::SaveAs => "另存为",
        DialogKind::Generic => "文件",
    }
}
