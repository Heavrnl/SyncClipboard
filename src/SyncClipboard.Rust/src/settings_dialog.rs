use crate::config::AppConfig;
use crate::log_info;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetStockObject, DEFAULT_GUI_FONT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY,
    HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_SZ,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetSystemMetrics, GetWindowTextLengthW, GetWindowTextW, LoadIconW, MessageBoxW, PostQuitMessage,
    RegisterClassW, SendMessageW, SetForegroundWindow, ShowWindow, TranslateMessage,
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, ES_AUTOHSCROLL,
    ES_PASSWORD, IDI_APPLICATION, MB_ICONINFORMATION, MB_ICONWARNING, MB_OK, MSG, SM_CXSCREEN,
    SM_CYSCREEN, SW_SHOW, WINDOW_STYLE, WNDCLASSW, WS_BORDER, WS_CAPTION, WS_CHILD, WS_MINIMIZEBOX,
    WS_SYSMENU, WS_TABSTOP, WS_VISIBLE, WM_COMMAND, WM_DESTROY, WM_SETFONT,
};

const ID_EDIT_URL: usize = 101;
const ID_EDIT_USER: usize = 102;
const ID_EDIT_PWD: usize = 103;
const ID_EDIT_INTERVAL: usize = 104;
const ID_CHECK_AUTOSTART: usize = 105;
const ID_BTN_SAVE: usize = 201;
const ID_BTN_CANCEL: usize = 202;

static IS_DIALOG_OPEN: AtomicBool = AtomicBool::new(false);
static CURRENT_CONFIG: Mutex<Option<AppConfig>> = Mutex::new(None);
static mut CONFIG_UPDATE_TX: Option<tokio::sync::mpsc::UnboundedSender<AppConfig>> = None;

static mut HWND_EDIT_URL: HWND = HWND(std::ptr::null_mut());
static mut HWND_EDIT_USER: HWND = HWND(std::ptr::null_mut());
static mut HWND_EDIT_PWD: HWND = HWND(std::ptr::null_mut());
static mut HWND_EDIT_INTERVAL: HWND = HWND(std::ptr::null_mut());
static mut HWND_CHECK_AUTOSTART: HWND = HWND(std::ptr::null_mut());

const RUN_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run\0";
const RUN_VALUENAME: &str = "SyncClipboardLite\0";

unsafe fn is_autostart_configured() -> bool {
    let subkey_w: Vec<u16> = RUN_SUBKEY.encode_utf16().collect();
    let name_w: Vec<u16> = RUN_VALUENAME.encode_utf16().collect();
    let mut hkey = HKEY::default();

    if RegOpenKeyExW(
        HKEY_CURRENT_USER,
        PCWSTR(subkey_w.as_ptr()),
        0,
        KEY_READ,
        &mut hkey,
    ).is_ok() {
        let res = RegQueryValueExW(
            hkey,
            PCWSTR(name_w.as_ptr()),
            None,
            None,
            None,
            None,
        );
        let _ = RegCloseKey(hkey);
        return res.is_ok();
    }
    false
}

unsafe fn set_autostart(enable: bool) {
    let subkey_w: Vec<u16> = RUN_SUBKEY.encode_utf16().collect();
    let name_w: Vec<u16> = RUN_VALUENAME.encode_utf16().collect();
    let mut hkey = HKEY::default();

    if enable {
        if let Ok(exe_path) = std::env::current_exe() {
            let path_str = format!("\"{}\"\0", exe_path.to_string_lossy());
            let path_w: Vec<u16> = path_str.encode_utf16().collect();
            let slice: &[u8] = std::slice::from_raw_parts(
                path_w.as_ptr() as *const u8,
                path_w.len() * 2,
            );

            if RegOpenKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey_w.as_ptr()),
                0,
                KEY_WRITE,
                &mut hkey,
            ).is_ok() {
                let _ = RegSetValueExW(
                    hkey,
                    PCWSTR(name_w.as_ptr()),
                    0,
                    REG_SZ,
                    Some(slice),
                );
                let _ = RegCloseKey(hkey);
            }
        }
    } else {
        if RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey_w.as_ptr()),
            0,
            KEY_WRITE,
            &mut hkey,
        ).is_ok() {
            let _ = RegDeleteValueW(hkey, PCWSTR(name_w.as_ptr()));
            let _ = RegCloseKey(hkey);
        }
    }
}

pub fn show_settings_dialog(
    config: AppConfig,
    config_update_tx: tokio::sync::mpsc::UnboundedSender<AppConfig>,
) {
    if IS_DIALOG_OPEN.load(Ordering::SeqCst) {
        return;
    }
    IS_DIALOG_OPEN.store(true, Ordering::SeqCst);

    {
        let mut cur = CURRENT_CONFIG.lock().unwrap();
        *cur = Some(config);
    }

    unsafe {
        CONFIG_UPDATE_TX = Some(config_update_tx);
    }

    std::thread::spawn(move || unsafe {
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class_name: Vec<u16> = "SyncClipSettingsClass\0".encode_utf16().collect();

        let wnd_class = WNDCLASSW {
            lpfnWndProc: Some(settings_wnd_proc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            ..Default::default()
        };

        RegisterClassW(&wnd_class);

        let win_w = 460;
        let win_h = 410;
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let pos_x = (screen_w - win_w) / 2;
        let pos_y = (screen_h - win_h) / 2;

        let title: Vec<u16> = "SyncClipboard Lite - 设置\0".encode_utf16().collect();
        let hwnd = CreateWindowExW(
            Default::default(),
            PCWSTR(class_name.as_ptr()),
            PCWSTR(title.as_ptr()),
            WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE,
            pos_x,
            pos_y,
            win_w,
            win_h,
            HWND(std::ptr::null_mut()),
            None,
            instance,
            None,
        );

        if let Ok(hwnd) = hwnd {
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);

            let mut msg = MSG::default();
            while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).0 > 0 {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        IS_DIALOG_OPEN.store(false, Ordering::SeqCst);
    });
}

unsafe extern "system" fn settings_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        windows::Win32::UI::WindowsAndMessaging::WM_CREATE => {
            init_controls(hwnd);
            LRESULT(0)
        }
        WM_COMMAND => {
            let cmd_id = (wparam.0 & 0xffff) as usize;
            match cmd_id {
                ID_BTN_SAVE => {
                    handle_save(hwnd);
                }
                ID_BTN_CANCEL => {
                    let _ = DestroyWindow(hwnd);
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

unsafe fn init_controls(hwnd: HWND) {
    let instance = GetModuleHandleW(None).unwrap_or_default();
    let font = GetStockObject(DEFAULT_GUI_FONT);

    let create_control = |cls: &str, text: &str, style: WINDOW_STYLE, x, y, w, h, id: usize| -> HWND {
        let cls_w: Vec<u16> = format!("{}\0", cls).encode_utf16().collect();
        let txt_w: Vec<u16> = format!("{}\0", text).encode_utf16().collect();
        let ctrl = CreateWindowExW(
            Default::default(),
            PCWSTR(cls_w.as_ptr()),
            PCWSTR(txt_w.as_ptr()),
            WS_CHILD | WS_VISIBLE | style,
            x,
            y,
            w,
            h,
            hwnd,
            windows::Win32::UI::WindowsAndMessaging::HMENU(id as *mut std::ffi::c_void),
            instance,
            None,
        ).unwrap();
        if !font.0.is_null() {
            SendMessageW(ctrl, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
        }
        ctrl
    };

    let cfg_lock = CURRENT_CONFIG.lock().unwrap();
    let cfg = cfg_lock.as_ref().cloned().unwrap_or_default();

    // 1. 服务器地址
    create_control("STATIC", "服务器地址 (Remote URL):", WINDOW_STYLE(0), 30, 20, 380, 20, 0);
    HWND_EDIT_URL = create_control("EDIT", &cfg.server_url, WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), 30, 45, 380, 25, ID_EDIT_URL);

    // 2. 用户名
    create_control("STATIC", "用户名 (UserName):", WINDOW_STYLE(0), 30, 80, 380, 20, 0);
    HWND_EDIT_USER = create_control("EDIT", &cfg.user_name, WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), 30, 105, 380, 25, ID_EDIT_USER);

    // 3. 密码
    create_control("STATIC", "密码 (Password):", WINDOW_STYLE(0), 30, 140, 380, 20, 0);
    HWND_EDIT_PWD = create_control("EDIT", &cfg.password, WS_BORDER | WS_TABSTOP | WINDOW_STYLE((ES_AUTOHSCROLL | ES_PASSWORD) as u32), 30, 165, 380, 25, ID_EDIT_PWD);

    // 4. 轮询间隔
    create_control("STATIC", "容灾轮询间隔 (秒):", WINDOW_STYLE(0), 30, 200, 380, 20, 0);
    let interval_str = cfg.pull_interval_sec.to_string();
    HWND_EDIT_INTERVAL = create_control("EDIT", &interval_str, WS_BORDER | WS_TABSTOP | WINDOW_STYLE(ES_AUTOHSCROLL as u32), 30, 225, 120, 25, ID_EDIT_INTERVAL);

    // 5. 开机自启动复选框 (Checkbox)
    HWND_CHECK_AUTOSTART = create_control(
        "BUTTON",
        "开机时自动启动 SyncClipboard Lite",
        WINDOW_STYLE(BS_AUTOCHECKBOX as u32) | WS_TABSTOP,
        30,
        268,
        380,
        24,
        ID_CHECK_AUTOSTART,
    );
    if is_autostart_configured() {
        SendMessageW(HWND_CHECK_AUTOSTART, BM_SETCHECK, WPARAM(1), LPARAM(0));
    }

    // 6. 按钮
    create_control("BUTTON", "保存并应用", WINDOW_STYLE(BS_DEFPUSHBUTTON as u32) | WS_TABSTOP, 180, 315, 110, 32, ID_BTN_SAVE);
    create_control("BUTTON", "取消", WINDOW_STYLE(BS_PUSHBUTTON as u32) | WS_TABSTOP, 305, 315, 105, 32, ID_BTN_CANCEL);
}

unsafe fn get_ctrl_text(hctrl: HWND) -> String {
    let len = GetWindowTextLengthW(hctrl);
    if len <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; (len + 1) as usize];
    GetWindowTextW(hctrl, &mut buf);
    if let Some(pos) = buf.iter().position(|&c| c == 0) {
        buf.truncate(pos);
    }
    String::from_utf16_lossy(&buf)
}

unsafe fn handle_save(hwnd: HWND) {
    let url = get_ctrl_text(HWND_EDIT_URL).trim().to_string();
    let user = get_ctrl_text(HWND_EDIT_USER).trim().to_string();
    let pwd = get_ctrl_text(HWND_EDIT_PWD).to_string();
    let interval_text = get_ctrl_text(HWND_EDIT_INTERVAL).trim().to_string();
    let interval = interval_text.parse::<u64>().unwrap_or(3).max(1);

    if url.is_empty() {
        let msg: Vec<u16> = "服务器地址不能为空！\0".encode_utf16().collect();
        let title: Vec<u16> = "提示\0".encode_utf16().collect();
        MessageBoxW(hwnd, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONWARNING);
        return;
    }

    // 1. 处理开机自启动复选框状态
    let check_state = SendMessageW(HWND_CHECK_AUTOSTART, BM_GETCHECK, WPARAM(0), LPARAM(0));
    let enable_autostart = check_state.0 == 1;
    set_autostart(enable_autostart);
    log_info(format!("[Config] 开机自启动状态已设置为: {}", enable_autostart));

    // 2. 保存新配置
    let new_cfg = AppConfig {
        server_url: url,
        user_name: user,
        password: pwd,
        pull_interval_sec: interval,
        enable_push: true,
        enable_pull: true,
    };

    if let Err(e) = new_cfg.save() {
        let msg: Vec<u16> = format!("保存配置失败: {}\0", e).encode_utf16().collect();
        let title: Vec<u16> = "错误\0".encode_utf16().collect();
        MessageBoxW(hwnd, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONWARNING);
        return;
    }

    log_info("[Config] 用户已更新配置并成功保存至 config.json");

    if let Some(ref tx) = CONFIG_UPDATE_TX {
        let _ = tx.send(new_cfg);
    }

    let msg: Vec<u16> = "配置保存成功，新配置已立即生效！\0".encode_utf16().collect();
    let title: Vec<u16> = "成功\0".encode_utf16().collect();
    MessageBoxW(hwnd, PCWSTR(msg.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONINFORMATION);

    let _ = DestroyWindow(hwnd);
}
