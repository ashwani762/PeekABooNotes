// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! peekaboo-mcp: lets an AI assistant (Claude, Gemini CLI, Codex, Cursor, VS Code, ...)
//! use PeekABoo Notes through the Model Context Protocol.
//!
//! The assistant starts this program and talks MCP to it on stdin/stdout, one JSON-RPC
//! message per line. It carries each line to the PeekABoo Notes app through a named pipe
//! and the app's answer back; the app does the work, so its windows stay up to date.
//! When the app isn't running, it is started (it lives beside this program). The app
//! only answers when "Allow assistants (MCP)" is on in its Settings.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// The app's pipe, its name and who serves it: the same code as the app's.
#[path = "../win/pipe.rs"]
mod pipe;

struct Link {
    to: File,
    from: BufReader<File>,
}

/// What `open` says when the pipe's server isn't this user's PeekABoo Notes.
const NOT_OURS: &str = "not this user's";

/// The app's pipe, if its server runs as this user: the app can tell who's asking, but
/// not act as them.
fn open() -> io::Result<Link> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::SECURITY_IDENTIFICATION;
    let name = pipe::pipe_name().ok_or_else(|| io::Error::other("couldn't tell who the user is"))?;
    let f = OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_IDENTIFICATION).open(name)?;
    if !pipe::served_by_me(f.as_raw_handle() as _) {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, NOT_OURS));
    }
    Ok(Link { to: f.try_clone()?, from: BufReader::new(f) })
}

/// Connect, starting the app first when it isn't running.
fn connect(started: &mut bool) -> Result<Link, String> {
    let begun = Instant::now();
    loop {
        // Just started, the app needs a moment to open its pipe; already running without
        // one, assistants are simply off, and there's no point waiting long.
        let patience = if *started { Duration::from_secs(10) } else { Duration::from_millis(1500) };
        match open() {
            Ok(l) => return Ok(l),
            // Someone else's program has the pipe: never talk to it.
            Err(e) if e.raw_os_error().is_none() && e.kind() == io::ErrorKind::PermissionDenied => {
                return Err("The pipe PeekABoo Notes uses is held by a program that isn't this user's PeekABoo Notes, so the bridge won't use it.".into());
            }
            Err(e) => {
                // 2: no such pipe (the app isn't running, or MCP is off); 231: all busy.
                if e.raw_os_error() == Some(2) && !*started && !app_running() {
                    *started = true;
                    let app = std::env::current_exe().map(|p| p.with_file_name("peekaboo-notes.exe")).map_err(|e| e.to_string())?;
                    start_app(&app).map_err(|e| format!("Couldn't start PeekABoo Notes ({}): {e}", app.display()))?;
                }
                if begun.elapsed() > patience {
                    return Err(if app_running() {
                        "PeekABoo Notes is running but isn't taking assistants: turn on \"Allow assistants (MCP)\" in its Settings, under Data.".into()
                    } else {
                        "PeekABoo Notes didn't start.".into()
                    });
                }
                std::thread::sleep(Duration::from_millis(150));
            }
        }
    }
}

/// Start the app on its own: inheriting no handles (the assistant's stdout among them,
/// which would keep it waiting for an end that never comes) and outside the assistant's
/// job, so closing the assistant doesn't close the app. Quietly: only the deck appears,
/// as at sign-in, and when two assistants start it at once the second copy just leaves.
fn start_app(app: &std::path::Path) -> io::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, CreateProcessW, DETACHED_PROCESS, PROCESS_INFORMATION, STARTUPINFOW};
    let exe: Vec<u16> = app.as_os_str().encode_wide_z();
    let dir: Vec<u16> = app.parent().map(|d| d.as_os_str().encode_wide_z()).unwrap_or_else(|| vec![0]);
    let line: Vec<u16> = std::ffi::OsString::from(format!("\"{}\" --quiet", app.display())).encode_wide_z();
    let try_with = |flags: u32| unsafe {
        let mut si: STARTUPINFOW = std::mem::zeroed();
        si.cb = size_of::<STARTUPINFOW>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        // CreateProcessW may write into the command line: its own copy each time.
        let mut cmd = line.clone();
        let ok = CreateProcessW(exe.as_ptr(), cmd.as_mut_ptr(), std::ptr::null(), std::ptr::null(), 0, flags, std::ptr::null(), dir.as_ptr(), &si, &mut pi);
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        CloseHandle(pi.hThread);
        CloseHandle(pi.hProcess);
        Ok(())
    };
    let flags = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // A job that doesn't allow breaking away refuses the first; then stay in it.
    try_with(flags | CREATE_BREAKAWAY_FROM_JOB).or_else(|_| try_with(flags))
}

trait WideZ {
    fn encode_wide_z(&self) -> Vec<u16>;
}

impl WideZ for std::ffi::OsStr {
    fn encode_wide_z(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

/// Whether the app is running for this user (its single-instance mutex exists).
fn app_running() -> bool {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
    let name: Vec<u16> = "Local\\PeekABooNotes.Singleton".encode_utf16().chain(Some(0)).collect();
    let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
    if h.is_null() {
        return false;
    }
    unsafe { CloseHandle(h) };
    true
}

/// Carry `line` over and bring the answer back. When the app went away before taking the
/// line, it reconnects and sends it once more; once the line has gone over, it never
/// sends it again (the app may have done it already: a note made twice is worse than an
/// error).
fn exchange(link: &mut Option<Link>, started: &mut bool, line: &str) -> Result<String, String> {
    for _ in 0..2 {
        if link.is_none() {
            *link = Some(connect(started)?);
        }
        let l = link.as_mut().unwrap();
        let sent = l.to.write_all(line.as_bytes()).and_then(|_| l.to.write_all(b"\n")).and_then(|_| l.to.flush());
        if sent.is_err() {
            *link = None;
            continue;
        }
        let mut answer = String::new();
        if l.from.read_line(&mut answer).map(|n| n > 0).unwrap_or(false) {
            return Ok(answer.trim_end().to_string());
        }
        *link = None;
        return Err("Lost the connection to PeekABoo Notes before it answered; the request may or may not have been carried out.".into());
    }
    Err("Lost the connection to PeekABoo Notes.".into())
}

fn main() {
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    let (mut link, mut started) = (None, false);
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        let line = line.trim().trim_start_matches('\u{FEFF}');
        if line.is_empty() {
            continue;
        }
        let reply = match exchange(&mut link, &mut started, line) {
            Ok(r) => r,
            Err(e) => {
                // Requests get an error; notifications get nothing.
                match serde_json::from_str::<Value>(line).ok().and_then(|m| m.get("id").cloned()) {
                    Some(id) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32000, "message": e } }).to_string(),
                    None => String::new(),
                }
            }
        };
        if !reply.is_empty() {
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}
