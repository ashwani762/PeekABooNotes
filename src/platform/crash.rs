//! Crash reporting: panics and native faults are written to `crash.log` in the data
//! folder (with a stack trace) instead of the process silently disappearing.

use std::io::Write;

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::Diagnostics::Debug::{EXCEPTION_POINTERS, SetUnhandledExceptionFilter};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW,
};

fn write(text: &str) {
    let path = crate::platform::util::data_dir().join("crash.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "==== {} ====\n{text}\n", crate::platform::util::stamp());
    }
}

fn module_of(addr: usize) -> String {
    unsafe {
        let mut m: HMODULE = std::ptr::null_mut();
        if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, addr as *const u16, &mut m) == 0 {
            return "?".into();
        }
        let mut buf = [0u16; 260];
        let n = GetModuleFileNameW(m, buf.as_mut_ptr(), 260) as usize;
        format!("{}+{:#x}", String::from_utf16_lossy(&buf[..n]), addr - m as usize)
    }
}

unsafe extern "system" fn filter(ep: *const EXCEPTION_POINTERS) -> i32 {
    unsafe {
        let rec = &*(*ep).ExceptionRecord;
        let addr = rec.ExceptionAddress as usize;
        let mut info = format!("native exception {:#010x} at {}", rec.ExceptionCode as u32, module_of(addr));
        if rec.NumberParameters >= 2 {
            info.push_str(&format!(" (access {} address {:#x})", if rec.ExceptionInformation[0] == 0 { "read" } else { "write" }, rec.ExceptionInformation[1]));
        }
        write(&format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
    }
    0 // EXCEPTION_CONTINUE_SEARCH: let Windows finish the crash as usual
}

/// Developer tracing: with PEEKABOO_TRACE set, append a line to %TEMP%\peekaboo-trace.txt.
pub fn trace(msg: &str) {
    if std::env::var_os("PEEKABOO_TRACE").is_none() {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(std::env::temp_dir().join("peekaboo-trace.txt")) {
        let _ = writeln!(f, "{:?} {msg}", std::time::SystemTime::now());
    }
}

pub fn install() {
    std::panic::set_hook(Box::new(|p| {
        write(&format!("panic: {p}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    unsafe { SetUnhandledExceptionFilter(Some(filter)) };
}
