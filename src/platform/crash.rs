// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! Crash reporting: a panic or native fault leaves a minidump in `logs\` (what a debugger
//! needs), the report in the app log, and `crash.log` in the data folder, instead of the
//! process silently disappearing.
//!
//! This runs in a dying process, often with its window still on screen, so it has to be
//! quick: the dump comes first, from a helper thread with a capped wait, and stacks are
//! listed as raw module+offset addresses. Symbolizing them here once froze a crash for
//! seconds (dbghelp goes looking for symbols); the dump can be symbolized later instead.

use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, HMODULE, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::Storage::FileSystem::{CREATE_ALWAYS, CreateFileW, FILE_ATTRIBUTE_NORMAL};
use windows_sys::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, MINIDUMP_EXCEPTION_INFORMATION, MINIDUMP_TYPE, MiniDumpWithThreadInfo, MiniDumpWithUnloadedModules,
    RtlCaptureStackBackTrace, SetUnhandledExceptionFilter,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW, GetProcAddress,
    LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::{
    CreateThread, GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId, GetExitCodeThread, WaitForSingleObject,
};

use super::log;
use super::util::wide;

/// Dumps kept in `logs\`; older ones are deleted.
const KEEP_DUMPS: usize = 3;
/// Longest the dying process waits for its dump.
const DUMP_WAIT_MS: u32 = 3000;

/// Only the first crash is reported: a fault while reporting one must not loop.
static CRASHED: AtomicBool = AtomicBool::new(false);

fn write(text: &str) {
    let path = super::util::data_dir().join("crash.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "==== {} ====\n{text}\n", super::util::stamp());
    }
}

/// "peekaboo-notes.exe+0x1a2b3"
fn module_of(addr: usize) -> String {
    unsafe {
        let mut m: HMODULE = std::ptr::null_mut();
        if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, addr as *const u16, &mut m) == 0 {
            return format!("{addr:#x}");
        }
        let mut buf = [0u16; 260];
        let n = GetModuleFileNameW(m, buf.as_mut_ptr(), 260) as usize;
        let path = String::from_utf16_lossy(&buf[..n]);
        let name = path.rsplit('\\').next().unwrap_or(&path);
        format!("{name}+{:#x}", addr - m as usize)
    }
}

/// The calling thread's stack as raw addresses. From an exception filter this walks on
/// through the exception dispatcher into the frames that faulted.
fn raw_stack() -> String {
    let mut frames = [std::ptr::null_mut::<c_void>(); 48];
    let n = unsafe { RtlCaptureStackBackTrace(1, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) } as usize;
    frames[..n].iter().map(|&f| format!("  at {}\n", module_of(f as usize))).collect()
}

type WriteDump = unsafe extern "system" fn(HANDLE, u32, HANDLE, MINIDUMP_TYPE, *const MINIDUMP_EXCEPTION_INFORMATION, *const c_void, *const c_void) -> i32;

struct DumpJob {
    path: Vec<u16>,
    thread: u32,
    ep: *const EXCEPTION_POINTERS,
}

unsafe extern "system" fn dump_thread(p: *mut c_void) -> u32 {
    unsafe {
        let job = &*(p as *const DumpJob);
        // Loaded only now: a static import would map dbghelp into every healthy session.
        let lib = LoadLibraryExW(wide("dbghelp.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        let Some(f) = (!lib.is_null()).then(|| GetProcAddress(lib, c"MiniDumpWriteDump".as_ptr().cast())).flatten() else { return 0 };
        let write_dump: WriteDump = std::mem::transmute(f);
        let file = CreateFileW(job.path.as_ptr(), GENERIC_WRITE, 0, std::ptr::null(), CREATE_ALWAYS, FILE_ATTRIBUTE_NORMAL, std::ptr::null_mut());
        if file == INVALID_HANDLE_VALUE {
            return 0;
        }
        let info = MINIDUMP_EXCEPTION_INFORMATION { ThreadId: job.thread, ExceptionPointers: job.ep as _, ClientPointers: 0 };
        let kind = MiniDumpWithThreadInfo | MiniDumpWithUnloadedModules; // stacks and modules: tens of KB
        let ok = write_dump(GetCurrentProcess(), GetCurrentProcessId(), file, kind, if job.ep.is_null() { std::ptr::null() } else { &info }, std::ptr::null(), std::ptr::null());
        CloseHandle(file);
        (ok != 0) as u32
    }
}

/// Write `logs\crash-YYYYMMDD-HHMMSS.dmp` for the calling thread's crash. From a helper
/// thread: dbghelp reads a stack best from outside it, and after a stack overflow there's
/// no room left on it anyway. The wait is capped, so a dump that can't finish (a lock the
/// crashed thread holds, say) never keeps the dying process on screen.
fn write_dump(ep: *const EXCEPTION_POINTERS) -> Option<PathBuf> {
    let dir = log::dir();
    let t = unsafe {
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    let path = dir.join(format!("crash-{:04}{:02}{:02}-{:02}{:02}{:02}.dmp", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond));
    // Leaked: if the wait times out the helper may still be reading it.
    let job = Box::into_raw(Box::new(DumpJob { path: wide(&path.to_string_lossy()), thread: unsafe { GetCurrentThreadId() }, ep }));
    let done = unsafe {
        let h = CreateThread(std::ptr::null(), 0, Some(dump_thread), job as *const c_void, 0, std::ptr::null_mut());
        if h.is_null() {
            return None;
        }
        let mut code = 0;
        let done = WaitForSingleObject(h, DUMP_WAIT_MS) == WAIT_OBJECT_0 && GetExitCodeThread(h, &mut code) != 0 && code != 0;
        CloseHandle(h);
        done
    };
    prune(&dir);
    done.then_some(path)
}

/// Keep the newest few dumps (their names sort by time).
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut dumps: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("crash-") && n.ends_with(".dmp")))
        .collect();
    dumps.sort();
    for old in dumps.iter().rev().skip(KEEP_DUMPS) {
        let _ = std::fs::remove_file(old);
    }
}

fn report(what: &str, ep: *const EXCEPTION_POINTERS, stack: impl FnOnce() -> String) {
    if CRASHED.swap(true, Ordering::SeqCst) {
        return;
    }
    let dump = match write_dump(ep) {
        Some(p) => format!("minidump: {}", p.display()),
        None => "minidump: not written".into(),
    };
    let text = format!("{what}\n{dump}\n{}", stack());
    log::error(&text);
    write(&text);
}

unsafe extern "system" fn filter(ep: *const EXCEPTION_POINTERS) -> i32 {
    let rec = unsafe { &*(*ep).ExceptionRecord };
    let mut info = format!("native exception {:#010x} at {}", rec.ExceptionCode as u32, module_of(rec.ExceptionAddress as usize));
    if rec.NumberParameters >= 2 {
        let access = match rec.ExceptionInformation[0] {
            0 => "read",
            8 => "execute",
            _ => "write",
        };
        info.push_str(&format!(" ({access} address {:#x})", rec.ExceptionInformation[1]));
    }
    report(&info, ep, raw_stack);
    0 // EXCEPTION_CONTINUE_SEARCH: let Windows finish the crash as usual (and report it)
}

/// Blank out quoted text in a panic message: std quotes the string it was slicing, and an
/// error's Debug output can quote strings too, which may be note text. Code-like spans
/// ("`Option::unwrap()`") stay readable.
fn redact(msg: &str) -> String {
    let mut out = String::with_capacity(msg.len());
    let mut rest = msg;
    while let Some(i) = rest.find(['`', '"']) {
        let q = rest[i..].chars().next().unwrap_or('"');
        out.push_str(&rest[..=i]);
        rest = &rest[i + 1..];
        let Some(j) = rest.find(q) else { break };
        let span = &rest[..j];
        let code = q == '`' && (span.contains("::") || span.ends_with("()")) && span.chars().all(|c| c.is_ascii_alphanumeric() || "_:()<>&".contains(c));
        out.push_str(if code { span } else { "..." });
        out.push(q);
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    out
}

/// Developer tracing (menu events and the like): a DEBUG line in the log, written only
/// with PEEKABOO_TRACE set.
pub fn trace(msg: &str) {
    log::debug(msg);
}

/// With PEEKABOO_CRASH=panic|fault|overflow, crash on purpose to test the reporting.
pub fn test_from_env() {
    let Some(kind) = std::env::var_os("PEEKABOO_CRASH") else { return };
    log::warn(&format!("PEEKABOO_CRASH={}: crashing on purpose", kind.to_string_lossy()));
    match kind.to_str() {
        Some("panic") => panic!("test panic (PEEKABOO_CRASH)"),
        Some("fault") => unsafe { std::ptr::write_volatile(std::hint::black_box(16usize) as *mut u8, 1) },
        Some("overflow") => {
            fn deeper(n: u64) -> u64 {
                let pad = std::hint::black_box([n; 64]);
                if pad[0] == u64::MAX { 0 } else { deeper(pad[0] + 1) + pad[63] }
            }
            std::hint::black_box(deeper(0));
        }
        _ => {}
    }
}

pub fn install() {
    std::panic::set_hook(Box::new(|p| {
        let place = p.location().map(|l| format!("{}:{}", l.file(), l.line())).unwrap_or_default();
        let msg = p.payload().downcast_ref::<&str>().copied().or(p.payload().downcast_ref::<String>().map(String::as_str)).unwrap_or("");
        let what = format!("panic at {place}: {}", redact(msg));
        // Symbolizing can be slow (see above), so only for developers who asked for detail.
        report(&what, std::ptr::null(), || if log::debug_enabled() { std::backtrace::Backtrace::force_capture().to_string() } else { raw_stack() });
    }));
    unsafe { SetUnhandledExceptionFilter(Some(filter)) };
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn panic_messages_lose_quoted_text() {
        assert_eq!(redact("byte index 3 is out of bounds of `my secret note`"), "byte index 3 is out of bounds of `...`");
        assert_eq!(redact("called `Option::unwrap()` on a `None` value"), "called `Option::unwrap()` on a `...` value");
        assert_eq!(redact("bad: Custom { msg: \"hello\" }"), "bad: Custom { msg: \"...\" }");
        assert_eq!(redact("unbalanced `quote"), "unbalanced `quote");
        assert_eq!(redact("plain"), "plain");
    }
}
