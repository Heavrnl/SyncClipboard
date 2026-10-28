use crate::clipboard::IS_WRITING_CLIPBOARD;
use crate::log_info;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, NIF_ICON, NIF_MESSAGE, NIF_TIP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW, DefWindowProcW,
    DestroyMenu, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW, GetSystemMetrics,
    LoadIconW, LoadImageW, PostQuitMessage, RegisterClassW, SetForegroundWindow, TrackPopupMenu,
    TranslateMessage, HICON, HMENU, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR,
    MF_GRAYED, MF_SEPARATOR, MF_STRING, MSG, SM_CXSMICON, SM_CYSMICON, TPM_BOTTOMALIGN, TPM_LEFTALIGN,
    WINDOW_STYLE, WNDCLASSW, WM_APP, WM_CLIPBOARDUPDATE, WM_COMMAND, WM_DESTROY, WM_RBUTTONUP,
};

const WM_TRAYICON: u32 = WM_APP + 1;
const ID_STATUS: usize = 1001;
const ID_SYNC_NOW: usize = 1002;
const ID_PAUSE_RESUME: usize = 1003;
const ID_SETTINGS: usize = 1004;
const ID_EXIT: usize = 1005;

pub static IS_PAUSED: AtomicBool = AtomicBool::new(false);

#[derive(Clone)]
pub struct TrayController {
    hwnd: HWND,
}

unsafe impl Send for TrayController {}
unsafe impl Sync for TrayController {}

impl TrayController {
    pub fn update_status(&self, status_text: &str) {
        unsafe {
            let tip: Vec<u16> = format!("SyncClipboard ({})\0", status_text).encode_utf16().collect();
            let mut nid = NOTIFYICONDATAW {
                cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: self.hwnd,
                uID: 1,
                uFlags: NIF_TIP,
                ..Default::default()
            };
            let copy_len = tip.len().min(nid.szTip.len() - 1);
            for i in 0..copy_len {
                nid.szTip[i] = tip[i];
            }
            nid.szTip[copy_len] = 0;
            let _ = Shell_NotifyIconW(NIM_MODIFY, &nid);
        }
    }
}

pub struct TrayApp {
    sync_trigger: tokio::sync::mpsc::UnboundedSender<()>,
    clip_trigger: tokio::sync::mpsc::UnboundedSender<()>,
    settings_trigger: tokio::sync::mpsc::UnboundedSender<()>,
}

static mut TRAY_APP: Option<TrayApp> = None;

const APP_ICO_BYTES: &[u8] = include_bytes!("../assets/app.ico");

unsafe fn load_tray_icon(instance: windows::Win32::Foundation::HMODULE) -> HICON {
    // 1. 尝试从构建嵌入的 PE 资源中加载 ID 1 的图标
    if let Ok(handle) = LoadImageW(
        instance,
        windows::core::PCWSTR(1 as *const u16),
        IMAGE_ICON,
        GetSystemMetrics(SM_CXSMICON),
        GetSystemMetrics(SM_CYSMICON),
        LR_DEFAULTCOLOR,
    ) {
        if !handle.0.is_null() {
            return HICON(handle.0);
        }
    }

    // 2. 从内嵌字节流解析并动态生成 HICON
    if let Some(hicon) = create_icon_from_ico_bytes(APP_ICO_BYTES) {
        return hicon;
    }

    // 3. 兜底默认图标
    LoadIconW(None, IDI_APPLICATION).unwrap_or_default()
}

unsafe fn create_icon_from_ico_bytes(bytes: &[u8]) -> Option<HICON> {
    if bytes.len() < 22 {
        return None;
    }
    let count = u16::from_le_bytes(bytes[4..6].try_into().ok()?) as usize;
    if count == 0 {
        return None;
    }

    let mut best_offset = 0;
    let mut best_size = 0;
    for i in 0..count {
        let entry_start = 6 + i * 16;
        if entry_start + 16 > bytes.len() {
            break;
        }
        let w = bytes[entry_start];
        let dw_bytes = u32::from_le_bytes(bytes[entry_start + 8..entry_start + 12].try_into().ok()?) as usize;
        let dw_offset = u32::from_le_bytes(bytes[entry_start + 12..entry_start + 16].try_into().ok()?) as usize;

        if (w == 32 || w == 16 || best_offset == 0) && dw_offset + dw_bytes <= bytes.len() {
            best_offset = dw_offset;
            best_size = dw_bytes;
            if w == 32 {
                break;
            }
        }
    }

    if best_size > 0 && best_offset + best_size <= bytes.len() {
        let icon_bits = &bytes[best_offset..best_offset + best_size];
        let hicon = CreateIconFromResourceEx(
            icon_bits,
            windows::Win32::Foundation::BOOL(1),
            0x00030000,
            GetSystemMetrics(SM_CXSMICON),
            GetSystemMetrics(SM_CYSMICON),
            LR_DEFAULTCOLOR,
        );
        if let Ok(h) = hicon {
            if !h.0.is_null() {
                return Some(h);
            }
        }
    }
    None
}

pub fn start_tray(
    sync_trigger: tokio::sync::mpsc::UnboundedSender<()>,
    clip_trigger: tokio::sync::mpsc::UnboundedSender<()>,
    settings_trigger: tokio::sync::mpsc::UnboundedSender<()>,
) -> TrayController {
    let (hwnd_tx, hwnd_rx) = std::sync::mpsc::channel::<isize>();

    std::thread::spawn(move || unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name: Vec<u16> = "SyncClipTrayWindow\0".encode_utf16().collect();

        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(tray_wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };

        RegisterClassW(&wnd_class);

        TRAY_APP = Some(TrayApp {
            sync_trigger,
            clip_trigger,
            settings_trigger,
        });

        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR("SyncClipTray\0".encode_utf16().collect::<Vec<u16>>().as_ptr()),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            HWND(std::ptr::null_mut()),
            None,
            instance,
            None,
        ).unwrap();

        let icon: HICON = load_tray_icon(instance.into());

        let tip: Vec<u16> = "SyncClipboard (运行中)\0".encode_utf16().collect();
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_TRAYICON,
            hIcon: icon,
            ..Default::default()
        };
        std::ptr::copy_nonoverlapping(tip.as_ptr(), nid.szTip.as_mut_ptr(), tip.len());
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);

        let add_res = AddClipboardFormatListener(hwnd);
        log_info(format!("[Tray] AddClipboardFormatListener hwnd = {:?}, res = {:?}", hwnd, add_res));

        let _ = hwnd_tx.send(hwnd.0 as isize);

        log_info("[Tray] 开始进入消息循环");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).0 > 0 {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        log_info("[Tray] 消息循环已退出");

        let _ = RemoveClipboardFormatListener(hwnd);
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    });

    let hwnd_val = hwnd_rx.recv().unwrap();
    TrayController {
        hwnd: HWND(hwnd_val as *mut std::ffi::c_void),
    }
}

unsafe extern "system" fn tray_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            log_info("[Tray] 收到 WM_CLIPBOARDUPDATE 事件");
            if !IS_WRITING_CLIPBOARD.load(Ordering::SeqCst) {
                if let Some(ref app) = TRAY_APP {
                    let _ = app.clip_trigger.send(());
                }
            }
            LRESULT(0)
        }
        WM_TRAYICON => {
            if lparam.0 as u32 == WM_RBUTTONUP {
                show_context_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd_id = (wparam.0 & 0xffff) as usize;
            match cmd_id {
                ID_SYNC_NOW => {
                    if let Some(ref app) = TRAY_APP {
                        let _ = app.sync_trigger.send(());
                    }
                }
                ID_PAUSE_RESUME => {
                    let cur = IS_PAUSED.load(Ordering::SeqCst);
                    IS_PAUSED.store(!cur, Ordering::SeqCst);
                }
                ID_SETTINGS => {
                    if let Some(ref app) = TRAY_APP {
                        let _ = app.settings_trigger.send(());
                    }
                }
                ID_EXIT => {
                    let _ = DestroyWindow(hwnd);
                    std::process::exit(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu: HMENU = CreatePopupMenu().unwrap();

    let status_str = if IS_PAUSED.load(Ordering::SeqCst) {
        "状态: 已暂停同步\0"
    } else {
        "状态: 同步中 (正常)\0"
    };
    let status_w: Vec<u16> = status_str.encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING | MF_GRAYED, ID_STATUS, PCWSTR(status_w.as_ptr()));

    let sep_w: Vec<u16> = "\0".encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(sep_w.as_ptr()));

    let sync_w: Vec<u16> = "立即手动同步\0".encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING, ID_SYNC_NOW, PCWSTR(sync_w.as_ptr()));

    let pause_str = if IS_PAUSED.load(Ordering::SeqCst) {
        "恢复同步\0"
    } else {
        "暂停同步\0"
    };
    let pause_w: Vec<u16> = pause_str.encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING, ID_PAUSE_RESUME, PCWSTR(pause_w.as_ptr()));

    let settings_w: Vec<u16> = "⚙️ 配置设置...\0".encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING, ID_SETTINGS, PCWSTR(settings_w.as_ptr()));

    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR(sep_w.as_ptr()));

    let exit_w: Vec<u16> = "退出\0".encode_utf16().collect();
    let _ = AppendMenuW(menu, MF_STRING, ID_EXIT, PCWSTR(exit_w.as_ptr()));

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);

    let _ = SetForegroundWindow(hwnd);
    let _ = TrackPopupMenu(
        menu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN,
        pt.x,
        pt.y,
        0,
        hwnd,
        None,
    );
    let _ = DestroyMenu(menu);
}
