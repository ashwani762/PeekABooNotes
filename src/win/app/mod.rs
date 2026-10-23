// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Application state, the hidden main window (tray icon + global hotkeys) and commands.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromRect};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, LoadLibraryW};
use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW};
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Controls::Dialogs::*;
use windows_sys::Win32::UI::Controls::{ICC_LISTVIEW_CLASSES, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{MOD_ALT, MOD_CONTROL, MOD_NOREPEAT, RegisterHotKey};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW, ShellExecuteW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::common::store::{Note, Store, title_of};
use crate::common::log;
use crate::win::util::{data_dir, exe_path, file_safe, wide};
use crate::win::{ui::{allnotes, deck, menu, note, settings, toast}, render::gfx, editor::rich, theme};

mod notes;
mod menus;
mod files;
mod prefs;
pub use notes::*;
pub use menus::*;
pub use files::*;
pub use prefs::*;

// Note commands (menus, shortcuts).
pub const NC_OPEN: u32 = 200;
pub const NC_PIN: u32 = 201;
pub const NC_ARCHIVE: u32 = 202;
pub const NC_DELETE: u32 = 203;
pub const NC_NEW: u32 = 204;
pub const NC_CLOSE: u32 = 205;
pub const NC_NEXT: u32 = 206;
pub const NC_PREV: u32 = 207;
pub const NC_CYCLE_COLOR: u32 = 208;
pub const NC_BOLD: u32 = 210;
pub const NC_ITALIC: u32 = 211;
pub const NC_UNDERLINE: u32 = 212;
pub const NC_STRIKE: u32 = 213;
pub const NC_BULLETS: u32 = 214;
pub const NC_NUMBERS: u32 = 215;
pub const NC_TASK: u32 = 216;
pub const NC_TOGGLE_CHECK: u32 = 217;
pub const NC_CUT: u32 = 218;
pub const NC_COPY: u32 = 219;
pub const NC_PASTE: u32 = 220;
pub const NC_SELECT_ALL: u32 = 221;
pub const NC_DECK: u32 = 222;
pub const NC_LINK: u32 = 223;
pub const NC_COLOR0: u32 = 300;

// App commands (tray menu, hotkeys).
const CMD_NEW: u32 = 1;
const CMD_ALL: u32 = 2;
const CMD_ARCHIVE: u32 = 3;
const CMD_REOPEN: u32 = 4;
const CMD_TOGGLE_DECK: u32 = 5;
const CMD_SWAP_EDGE: u32 = 6;
const CMD_UNDO: u32 = 14;
const CMD_QUIT: u32 = 15;
const CMD_SETTINGS: u32 = 16;

const MAIN_CLASS: &str = "PeekABooMain";
const WM_TRAY: u32 = WM_APP + 1;
pub const WM_ACTIVATE_OTHER: u32 = WM_APP + 2;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// The startup entry (shown in Task Manager).
const RUN_VALUE: &str = "PeekABoo Notes";

pub struct Ui {
    pub hinst: HINSTANCE,
    pub main: HWND,
    pub worker: HWND,
    pub notes: HashMap<u64, HWND>,
    pub pending: Vec<(usize, Note)>,
    pub dark: bool,
    pub icon: HICON,
    pub icon_sm: HICON,
    pub taskbar_created: u32,
}

thread_local! {
    static STORE: RefCell<Option<Store>> = const { RefCell::new(None) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

pub fn store<R>(f: impl FnOnce(&Store) -> R) -> R {
    STORE.with(|s| f(s.borrow().as_ref().expect("store not loaded")))
}

pub fn store_mut<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    STORE.with(|s| f(s.borrow_mut().as_mut().expect("store not loaded")))
}

pub fn save() {
    store(|s| s.save());
}

pub fn ui<R>(f: impl FnOnce(&mut Ui) -> R) -> R {
    UI.with(|u| f(u.borrow_mut().as_mut().expect("ui not initialised")))
}

pub fn hinst() -> HINSTANCE {
    ui(|u| u.hinst)
}

pub fn dark() -> bool {
    ui(|u| u.dark)
}

pub fn register_class(name: &str, proc_: WNDPROC, cursor: *const u16) {
    let (hinst, icon, icon_sm) = ui(|u| (u.hinst, u.icon, u.icon_sm));
    let cls = wide(name);
    unsafe {
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: proc_,
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinst,
            hIcon: icon,
            hCursor: LoadCursorW(null_mut(), cursor),
            hbrBackground: null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: cls.as_ptr(),
            hIconSm: icon_sm,
        };
        RegisterClassExW(&wc); // fails harmlessly if already registered
    }
}

// ---------------------------------------------------------------- note actions

// ---------------------------------------------------------------- menus

// Fluent icon glyphs used in menus.
const IC_ADD: &str = "\u{E710}";
const IC_OPEN: &str = "\u{E8A7}";
const IC_LIST: &str = "\u{E8FD}";
const IC_PIN: &str = "\u{E718}";
const IC_DELETE: &str = "\u{E74D}";
const IC_CUT: &str = "\u{E8C6}";
const IC_COPY: &str = "\u{E8C8}";
const IC_PASTE: &str = "\u{E77F}";
const IC_SELECT: &str = "\u{E8B3}";
const IC_BACK: &str = "\u{E72B}";
const IC_FORWARD: &str = "\u{E72A}";
const IC_UNDO: &str = "\u{E7A7}";
const IC_REFRESH: &str = "\u{E72C}";
const IC_POWER: &str = "\u{E7E8}";
const IC_SETTINGS: &str = "\u{E713}";
const IC_ARCHIVE: &str = "\u{E7B8}";
const IC_LINK: &str = "\u{E71B}";
const IC_SWAP: &str = "\u{E8AB}";

// ---------------------------------------------------------------- import / export / startup

// ---------------------------------------------------------------- tray + main window

/// The tray tooltip in the current language.
pub fn tray_relabel() {
    tray_icon(NIM_MODIFY);
}

fn tray(add: bool) {
    tray_icon(if add { NIM_ADD } else { NIM_DELETE });
}

fn tray_icon(op: u32) {
    let (main, icon) = ui(|u| (u.main, u.icon_sm));
    unsafe {
        let mut nid: NOTIFYICONDATAW = std::mem::zeroed();
        nid.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        nid.hWnd = main;
        nid.uID = 1;
        nid.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
        nid.uCallbackMessage = WM_TRAY;
        nid.hIcon = icon;
        for (i, c) in t!("tray_tooltip", t!("app_name")).encode_utf16().take(127).enumerate() {
            nid.szTip[i] = c;
        }
        Shell_NotifyIconW(op, &nid);
    }
}

const HOTKEYS: [(u32, u8); 6] = [
    (CMD_NEW, b'N'),
    (CMD_ALL, b'A'),
    (CMD_ARCHIVE, b'L'),
    (CMD_REOPEN, b'O'),
    (CMD_TOGGLE_DECK, b'H'),
    (CMD_SWAP_EDGE, b'E'),
];

pub fn quit() {
    let wins: Vec<HWND> = ui(|u| u.notes.values().copied().collect());
    // Before the teardown, so a quit that stalls or crashes on the way out shows up.
    log::info(&format!("quitting, {} note windows open", wins.len()));
    for h in wins {
        unsafe { DestroyWindow(h) }; // flushes in WM_DESTROY
    }
    for w in [allnotes::hwnd(), settings::hwnd()] {
        if !w.is_null() {
            unsafe { DestroyWindow(w) };
        }
    }
    toast::close();
    crate::win::ui::ghost::clear();
    deck::shutdown();
    tray(false);
    save();
    // Release Direct2D/DirectWrite while the process is fully alive: left to the exit-time
    // thread-local teardown, Direct2D faults inside the loader's shutdown and the exit stalls.
    crate::win::render::dw::trim();
    crate::win::editor::emoji::trim();
    unsafe { PostQuitMessage(0) };
}

unsafe extern "system" fn main_proc(h: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tb = UI.with(|u| u.try_borrow().ok().and_then(|u| u.as_ref().map(|u| u.taskbar_created))).unwrap_or(0);
    match msg {
        WM_HOTKEY => command(wp as u32),
        WM_TRAY => match (lp & 0xFFFF) as u32 {
            WM_LBUTTONUP => show_all_notes(false),
            WM_RBUTTONUP | WM_CONTEXTMENU => tray_menu(),
            _ => {}
        },
        WM_ACTIVATE_OTHER => show_all_notes(false),
        WM_SETTINGCHANGE => {
            apply_motion();
            apply_theme();
            deck::refresh();
        }
        WM_DISPLAYCHANGE => deck::refresh(),
        WM_TIMER if wp == T_TRIM => {
            unsafe { KillTimer(h, T_TRIM) };
            trim_memory();
        }
        WM_QUERYENDSESSION => return 1,
        WM_ENDSESSION if wp != 0 => {
            let wins: Vec<HWND> = ui(|u| u.notes.values().copied().collect());
            for w in wins {
                note::flush(w);
            }
            save();
            log::shutdown("Windows is signing out or shutting down");
        }
        _ if msg == tb && tb != 0 => tray(true),
        _ => return unsafe { DefWindowProcW(h, msg, wp, lp) },
    }
    0
}

const T_TRIM: usize = 1;

/// A window closed: once only the deck is left (and has been for a moment), hand memory
/// back to Windows.
pub fn schedule_trim() {
    let h = ui(|u| u.main);
    if !h.is_null() {
        unsafe { SetTimer(h, T_TRIM, 3000, None) };
    }
}

fn trim_memory() {
    let busy = ui(|u| !u.notes.is_empty()) || !crate::win::ui::allnotes::hwnd().is_null() || !crate::win::ui::settings::hwnd().is_null();
    if busy {
        return;
    }
    crate::win::render::dw::trim();
    crate::win::editor::emoji::trim();
    unsafe {
        use windows_sys::Win32::System::Memory::{GetProcessHeap, HeapCompact};
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
        HeapCompact(GetProcessHeap(), 0);
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

pub fn run() -> i32 {
    unsafe {
        // One instance only: a second launch just brings up All Notes.
        let mutex = CreateMutexW(std::ptr::null(), 1, wide("Local\\PeekABooNotes.Singleton").as_ptr());
        if windows_sys::Win32::Foundation::GetLastError() == windows_sys::Win32::Foundation::ERROR_ALREADY_EXISTS {
            let other = FindWindowW(wide(MAIN_CLASS).as_ptr(), std::ptr::null());
            if !other.is_null() {
                PostMessageW(other, WM_ACTIVATE_OTHER, 0, 0);
            }
            return 0;
        }
        let _ = mutex;
        log::startup();

        // OLE: RichEdit's drag and drop (text within and between notes, files dropped in)
        // and dragging attachments out.
        let _ = windows::Win32::System::Ole::OleInitialize(None);
        LoadLibraryW(wide("Msftedit.dll").as_ptr());
        let icc = INITCOMMONCONTROLSEX { dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES | ICC_STANDARD_CLASSES };
        InitCommonControlsEx(&icc);
        theme::allow_dark_menus();

        STORE.with(|s| *s.borrow_mut() = Some(Store::load(data_dir())));
        crate::common::i18n::init(&store(|s| s.d.settings.language.clone()));
        log::info(&format!("language {}", crate::common::i18n::tag()));
        crate::win::crash::test_from_env();
        apply_motion();
        let (font, size) = store(|s| (s.d.settings.note_font, s.d.settings.note_size));
        rich::set_typography(font, size);
        let hinst = GetModuleHandleW(std::ptr::null());
        UI.with(|u| {
            *u.borrow_mut() = Some(Ui {
                hinst,
                main: null_mut(),
                worker: null_mut(),
                notes: HashMap::new(),
                pending: Vec::new(),
                dark: wanted_dark(),
                icon: gfx::app_icon(GetSystemMetrics(SM_CXICON).max(32)),
                icon_sm: gfx::app_icon(GetSystemMetrics(SM_CXSMICON).max(16)),
                taskbar_created: RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()),
            })
        });

        register_class(MAIN_CLASS, Some(main_proc), IDC_ARROW);
        let main = CreateWindowExW(WS_EX_TOOLWINDOW, wide(MAIN_CLASS).as_ptr(), wide(t!("app_name")).as_ptr(), WS_POPUP, 0, 0, 0, 0, null_mut(), null_mut(), hinst, null_mut());
        let worker = CreateWindowExW(
            0,
            wide(rich::CLASS).as_ptr(),
            std::ptr::null(),
            WS_CHILD | (ES_MULTILINE as u32),
            0,
            0,
            100,
            100,
            main,
            null_mut(),
            hinst,
            null_mut(),
        );
        rich::setup(worker);
        rich::apply_base_format(worker, 0x000000, 0xFFFFFF);
        ui(|u| {
            u.main = main;
            u.worker = worker;
        });

        note::register();
        deck::create();
        tray(true);
        for (id, key) in HOTKEYS {
            // If another app owns a combination we simply go without it.
            RegisterHotKey(main, id as i32, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, key as u32);
        }

        while pump_once() {}
        log::shutdown("quit");
        0
    }
}

/// One turn of the message loop. Idle: block in GetMessage (0% CPU). Animating: drain
/// input, step every spring, then wait for the compositor's next frame, so motion runs at
/// the display's refresh rate. Returns false once WM_QUIT arrives (nested loops, such as
/// menus, re-post it so the outer loop sees it too).
pub fn pump_once() -> bool {
    unsafe {
        let mut msg: MSG = std::mem::zeroed();
        if crate::win::render::anim::active() {
            while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
                if msg.message == WM_QUIT {
                    PostQuitMessage(msg.wParam as i32);
                    return false;
                }
                dispatch(&msg);
            }
            crate::win::render::anim::tick();
            if crate::win::render::anim::active() {
                crate::win::render::anim::wait_vsync();
            }
        } else {
            if GetMessageW(&mut msg, null_mut(), 0, 0) <= 0 {
                PostQuitMessage(0);
                return false;
            }
            dispatch(&msg);
        }
        true
    }
}

fn dispatch(msg: &MSG) {
    if allnotes::pre_translate(msg) {
        return;
    }
    unsafe {
        TranslateMessage(msg);
        DispatchMessageW(msg);
    }
}
