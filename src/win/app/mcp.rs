// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP for assistants, on Windows: a named pipe the `peekaboo-mcp` program connects to,
//! and the note work behind `common::mcp`'s tools. Each client gets a thread that reads
//! its lines; every request is carried out on the UI thread (queued, then a message to
//! the main window), so an assistant's edits behave like the user's: open notes update,
//! the deck and All Notes refresh, nothing races the store.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{CloseHandle, ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE, LocalFree};
use windows_sys::Win32::Security::Authorization::{ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1};
use windows_sys::Win32::Security::{GetTokenInformation, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use super::*;
use crate::common::mcp::{self, State, Status};
use crate::common::store::title_of;
use crate::common::{attach, search};

/// Posted to the main window when a request is queued. It carries nothing: the work is
/// in `QUEUE`, so a message from anywhere else at most finds the queue empty.
pub const WM_MCP: u32 = WM_APP + 77;

/// A request line, and where its reply goes.
type Job = (String, mpsc::Sender<String>);
static QUEUE: Mutex<VecDeque<Job>> = Mutex::new(VecDeque::new());

/// Connected assistants' pipe handles, so turning assistants off can let them go at
/// once. A client takes its own out (under the lock) before its handle closes.
static CLIENTS_OPEN: Mutex<Vec<isize>> = Mutex::new(Vec::new());

/// The longest request line taken: a note's 100,000 characters with room to spare. A
/// longer one is answered with an error, never held in memory whole.
const MAX_LINE: usize = 4 << 20;

/// The pipe's name: one per Windows user. (`peekaboo-mcp` builds the same name.)
fn pipe_name() -> String {
    format!(r"\\.\pipe\PeekABooNotes.mcp.{}", std::env::var("USERNAME").unwrap_or_default())
}

/// Settings > Data > "Allow assistants": off until the user turns it on.
static ENABLED: AtomicBool = AtomicBool::new(false);
static LISTENING: AtomicBool = AtomicBool::new(false);
static MAIN: AtomicIsize = AtomicIsize::new(0);

/// The app's main window, which requests are carried out on.
pub fn init(main: HWND) {
    MAIN.store(main as isize, Ordering::SeqCst);
    set_enabled(store(|s| s.d.settings.mcp));
}

/// Let assistants in, or stop: the pipe closes and connected ones are let go.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
    if on {
        listen();
        log::info("mcp: on");
    } else {
        if LISTENING.load(Ordering::SeqCst) {
            // The server waits for a connection: make one, so it sees it's off and stops.
            let _ = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name());
        }
        // And the assistants connected now are let go.
        for &h in CLIENTS_OPEN.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            unsafe { DisconnectNamedPipe(h as HANDLE) };
        }
        log::info("mcp: off");
    }
}

/// Start the server, unless one is running.
fn listen() {
    if !LISTENING.swap(true, Ordering::SeqCst) {
        let main = MAIN.load(Ordering::SeqCst);
        if std::thread::Builder::new().name("mcp".into()).spawn(move || serve(main)).is_err() {
            LISTENING.store(false, Ordering::SeqCst);
        }
    }
}

/// Where `peekaboo-mcp.exe` is (beside the app).
pub fn bridge_path() -> std::path::PathBuf {
    crate::win::util::exe_path().with_file_name("peekaboo-mcp.exe")
}

/// The command an assistant runs for the bridge. From the Store/MSIX package it is a
/// command on PATH (an execution alias); its real folder changes with every update.
fn bridge_command() -> String {
    if packaged() { "peekaboo-mcp.exe".to_string() } else { bridge_path().to_string_lossy().into_owned() }
}

/// Assistant apps that run local MCP servers, as the Settings menu lists them.
pub const CLIENTS: [&str; 9] = ["ChatGPT (desktop app)", "Claude Desktop", "Claude Code", "Gemini CLI", "Codex", "Cursor", "VS Code", "Windsurf", "LM Studio"];

/// What to paste for assistant app `i` of `CLIENTS`: a command to run, or a piece of its
/// MCP settings file.
pub fn setup_for(i: usize) -> String {
    let exe = bridge_command();
    // The usual "mcpServers" JSON (Claude Desktop, Gemini CLI, Cursor, Windsurf, LM Studio).
    let servers = || serde_json::to_string_pretty(&json!({ "mcpServers": { "peekaboo-notes": { "command": exe } } })).unwrap_or_default();
    match CLIENTS.get(i).copied().unwrap_or("") {
        // Its "Add MCP server" form (an STDIO server) takes just the command.
        "ChatGPT (desktop app)" => exe,
        "Claude Code" => format!("claude mcp add peekaboo-notes -- \"{exe}\""),
        // TOML for ~/.codex/config.toml; a literal string, so the backslashes stay.
        "Codex" => format!("[mcp_servers.peekaboo-notes]\ncommand = '{exe}'"),
        "VS Code" => serde_json::to_string_pretty(&json!({ "servers": { "peekaboo-notes": { "type": "stdio", "command": exe } } })).unwrap_or_default(),
        _ => servers(),
    }
}

/// What `connect` did for an assistant app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Connect {
    /// Its settings name the bridge now; it has to be restarted to see it.
    Done,
    /// It takes the setup in a form of its own: the setup is on the clipboard.
    Copied,
    /// It isn't installed here (its settings folder is missing): the setup is on the
    /// clipboard.
    Missing,
    /// Its settings couldn't be read or written: the setup is on the clipboard.
    Failed,
}

/// Set assistant app `i` of `CLIENTS` up to run the bridge, in its own MCP settings
/// (a backup of the file is kept beside it); the app then has to be restarted. Where
/// that can't be done, the setup goes on the clipboard, as `setup_for` gives it.
pub fn connect(i: usize) -> Connect {
    let exe = bridge_command();
    let home = |rel: &str| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join(rel));
    let roaming = |rel: &str| std::env::var_os("APPDATA").map(|h| std::path::PathBuf::from(h).join(rel));
    let server = json!({ "command": exe });
    let outcome = match CLIENTS.get(i).copied().unwrap_or("") {
        "Claude Desktop" => roaming(r"Claude\claude_desktop_config.json").map(|p| put_json(&p, "mcpServers", server)),
        // Its own command keeps its settings file in step with a running Claude Code.
        "Claude Code" => Some(if run_hidden(&["claude", "mcp", "add", "-s", "user", "peekaboo-notes", "--", &exe]) { Connect::Done } else { Connect::Missing }),
        "Gemini CLI" => home(r".gemini\settings.json").map(|p| put_json(&p, "mcpServers", server)),
        "Codex" => home(r".codex\config.toml").map(|p| put_toml(&p, &setup_for(i))),
        "Cursor" => home(r".cursor\mcp.json").map(|p| put_json(&p, "mcpServers", server)),
        "VS Code" => roaming(r"Code\User\mcp.json").map(|p| put_json(&p, "servers", json!({ "type": "stdio", "command": exe }))),
        "Windsurf" => home(r".codeium\windsurf\mcp_config.json").map(|p| put_json(&p, "mcpServers", server)),
        "LM Studio" => home(r".lmstudio\mcp.json").map(|p| put_json(&p, "mcpServers", server)),
        _ => None,
    };
    let outcome = outcome.unwrap_or(Connect::Copied);
    if outcome != Connect::Done {
        crate::win::util::set_clipboard_text(&setup_for(i));
    }
    outcome
}

/// Put `entry` as "peekaboo-notes" in the `section` object of the JSON file at `path`,
/// keeping everything else in it. Missing when its folder (the app) isn't there.
fn put_json(path: &std::path::Path, section: &str, entry: Value) -> Connect {
    let Some(dir) = path.parent() else { return Connect::Failed };
    if !dir.is_dir() {
        return Connect::Missing;
    }
    let mut root = match std::fs::read_to_string(path) {
        Ok(s) if !s.trim().is_empty() => match serde_json::from_str::<Value>(&s) {
            Ok(v) => v,
            Err(_) => return Connect::Failed,
        },
        _ => json!({}),
    };
    let Some(obj) = root.as_object_mut() else { return Connect::Failed };
    let s = obj.entry(section).or_insert_with(|| json!({}));
    let Some(s) = s.as_object_mut() else { return Connect::Failed };
    s.insert("peekaboo-notes".to_string(), entry);
    write_back(path, &serde_json::to_string_pretty(&root).unwrap_or_default())
}

/// Put the TOML `table` (its header line first) in the file at `path`, in place of the
/// same table if it's there, keeping everything else.
fn put_toml(path: &std::path::Path, table: &str) -> Connect {
    let Some(dir) = path.parent() else { return Connect::Failed };
    if !dir.is_dir() {
        return Connect::Missing;
    }
    let head = table.lines().next().unwrap_or_default();
    let cur = std::fs::read_to_string(path).unwrap_or_default();
    let next = match cur.find(head) {
        Some(i) => {
            let after = i + head.len();
            let end = cur[after..].find("\n[").map_or(cur.len(), |j| after + j + 1);
            format!("{}{table}\n{}", &cur[..i], &cur[end..])
        }
        None => format!("{cur}{}{table}\n", if cur.is_empty() || cur.ends_with('\n') { "" } else { "\n" }),
    };
    write_back(path, &next)
}

/// Write `text` to `path`, keeping what was there as `<name>.bak`.
fn write_back(path: &std::path::Path, text: &str) -> Connect {
    if path.is_file() {
        let mut bak = path.as_os_str().to_owned();
        bak.push(".bak");
        let _ = std::fs::copy(path, bak);
    }
    match std::fs::write(path, text) {
        Ok(()) => Connect::Done,
        Err(_) => Connect::Failed,
    }
}

/// Run a command line without a console window; true when it succeeded.
fn run_hidden(args: &[&str]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("cmd").arg("/c").args(args).creation_flags(CREATE_NO_WINDOW).output().is_ok_and(|o| o.status.success())
}

/// An assistant's change is only done when it reached the disk: say so when it didn't,
/// rather than report success for notes that a restart would lose.
fn saved_or_err() -> Result<(), String> {
    if save_failed() {
        Err("The change is in the app but couldn't be saved to disk (the notes file is locked or can't be written). It will be saved when that's fixed; tell the user.".into())
    } else {
        Ok(())
    }
}

fn serve(main: isize) {
    serve_while_on(main);
    LISTENING.store(false, Ordering::SeqCst);
    // Turned off and straight back on while this was on its way out: carry on.
    if ENABLED.load(Ordering::SeqCst) {
        listen();
    }
}

fn serve_while_on(main: isize) {
    let name = wide(&pipe_name());
    let sd = OwnerOnly::new();
    let mut failures = 0;
    loop {
        let sa = SECURITY_ATTRIBUTES { nLength: size_of::<SECURITY_ATTRIBUTES>() as u32, lpSecurityDescriptor: sd.as_ref().map_or(std::ptr::null_mut(), |d| d.0), bInheritHandle: 0 };
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                if sd.is_some() { &sa } else { std::ptr::null() },
            )
        };
        if h == INVALID_HANDLE_VALUE {
            // Taken for a moment (another program holding the name, a handle shortage):
            // try again for a while rather than leave assistants off for the session.
            failures += 1;
            log::warn(&format!("mcp: couldn't open the pipe ({})", unsafe { GetLastError() }));
            if failures > 10 || !ENABLED.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_secs(failures));
            continue;
        }
        failures = 0;
        let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        let file = unsafe { File::from_raw_handle(h as _) };
        if !ENABLED.load(Ordering::SeqCst) {
            return;
        }
        if ok {
            let _ = std::thread::Builder::new().name("mcp client".into()).spawn(move || client(main, file));
        }
    }
}

/// A security descriptor that lets only the user running the app open the pipe (by
/// default anyone on the computer could read it).
struct OwnerOnly(PSECURITY_DESCRIPTOR);

impl OwnerOnly {
    fn new() -> Option<OwnerOnly> {
        let sid = user_sid()?;
        let sddl = wide(&format!("D:P(A;;GA;;;{sid})"));
        let mut sd: PSECURITY_DESCRIPTOR = std::ptr::null_mut();
        let ok = unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut sd, std::ptr::null_mut()) } != 0;
        ok.then_some(OwnerOnly(sd))
    }
}

impl Drop for OwnerOnly {
    fn drop(&mut self) {
        unsafe { LocalFree(self.0 as _) };
    }
}

/// The running user's SID as text ("S-1-5-21-...").
fn user_sid() -> Option<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut buf = vec![0u8; 256];
        let mut len = 0u32;
        let got = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as _, buf.len() as u32, &mut len) != 0;
        CloseHandle(token);
        if !got {
            return None;
        }
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text: windows_sys::core::PWSTR = std::ptr::null_mut();
        if ConvertSidToStringSidW(user.User.Sid, &mut text) == 0 {
            return None;
        }
        let n = (0..).take_while(|&i| *text.add(i) != 0).count();
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(text, n));
        LocalFree(text as _);
        Some(s)
    }
}

/// The next line from `r`, without its newline: Ok(None) at the end, Err(()) for a line
/// longer than `MAX_LINE` (which is read past, never kept).
fn read_line(r: &mut impl BufRead) -> std::io::Result<Option<Result<String, ()>>> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && buf.len() > MAX_LINE {
        // Too long: skip to its end.
        loop {
            buf.clear();
            let m = r.by_ref().take(64 * 1024).read_until(b'\n', &mut buf)?;
            if m == 0 || buf.last() == Some(&b'\n') {
                break;
            }
        }
        return Ok(Some(Err(())));
    }
    while matches!(buf.last(), Some(b'\n' | b'\r')) {
        buf.pop();
    }
    Ok(Some(Ok(String::from_utf8_lossy(&buf).into_owned())))
}

fn client(main: isize, file: File) {
    log::info("mcp: an assistant connected");
    let handle = file.as_raw_handle() as isize;
    CLIENTS_OPEN.lock().unwrap_or_else(|e| e.into_inner()).push(handle);
    if let Ok(out) = file.try_clone() {
        talk(main, BufReader::new(file), out);
    }
    CLIENTS_OPEN.lock().unwrap_or_else(|e| e.into_inner()).retain(|&h| h != handle);
    log::info("mcp: the assistant disconnected");
}

fn talk(main: isize, mut from: BufReader<File>, mut out: File) {
    while let Ok(Some(line)) = read_line(&mut from) {
        if !ENABLED.load(Ordering::SeqCst) {
            break;
        }
        let reply = match line {
            Err(()) => mcp::request_too_long(),
            Ok(l) if l.trim().is_empty() => continue,
            Ok(l) => {
                let (tx, rx) = mpsc::channel();
                QUEUE.lock().unwrap_or_else(|e| e.into_inner()).push_back((l, tx));
                unsafe { PostMessageW(main as HWND, WM_MCP, 0, 0) };
                // The app gone (quitting) or stuck: let the assistant go.
                match rx.recv_timeout(Duration::from_secs(120)) {
                    Ok(r) => r,
                    Err(_) => break,
                }
            }
        };
        // One line back for every line in; empty for a notification.
        if out.write_all(reply.as_bytes()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
}

/// `WM_MCP`, on the UI thread: carry out what's queued.
pub fn on_message() {
    loop {
        let Some((line, reply)) = QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front() else { break };
        let _ = reply.send(mcp::handle(&line, &mut WinNotes).unwrap_or_default());
    }
}

// ---------------------------------------------------------------- the work

struct WinNotes;

fn when(ts: i64) -> String {
    let t = crate::os::local_time(ts);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
}

/// Everything after the title line as plain text (what search looks in).
fn rest_of(text: &str) -> String {
    crate::common::store::search_rest(text)
}

fn summary(n: &crate::common::store::Note) -> Value {
    let preview: String = rest_of(&n.text).lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect();
    json!({
        "id": n.id,
        "title": title_of(&n.text),
        "preview": preview,
        "color": mcp::color_name(n.color),
        "pinned": n.pinned,
        "archived": n.archived,
        "in_deck": !n.off_deck && !n.archived,
        "updated": when(n.updated),
    })
}

/// Save what's typed in an open note first, so an assistant reads (and builds on) it.
fn flush_open(id: u64) {
    if let Some(h) = note_hwnd(id) {
        note::flush(h);
    }
    if let Some(h) = crate::win::ui::allnotes::editor_for(id) {
        note::flush(h);
    }
}

/// A RichEdit to build content in, set up like the worker; gone when dropped.
struct Scratch(HWND);

impl Scratch {
    fn new() -> Scratch {
        let e = unsafe {
            CreateWindowExW(0, wide(rich::CLASS).as_ptr(), std::ptr::null(), WS_CHILD | (ES_MULTILINE as u32), 0, 0, 100, 100, ui(|u| u.main), null_mut(), ui(|u| u.hinst), null_mut())
        };
        rich::setup(e);
        rich::apply_base_format(e, 0x000000, 0xFFFFFF);
        Scratch(e)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        unsafe { DestroyWindow(self.0) };
    }
}

/// Markdown into editor `e`, with files it links on this computer copied in.
fn load(e: HWND, md: &str) {
    rich::load_markdown(e, md);
    note::adopt_files(e, &crate::common::util::data_dir());
}

/// (text, rtf) for a note from editor `e`'s content.
fn take(e: HWND) -> (String, String) {
    (rich::to_markdown(e, false), rich::get_rtf(e))
}

/// The attachments in editor `e`: names and where they are.
fn attachments(e: HWND) -> Vec<Value> {
    let t = rich::text(e);
    let mut seen = Vec::new();
    for (_, _, url) in rich::instructions(&t, 0, t.len()) {
        if let Some(name) = attach::name_of(&url)
            && !seen.contains(&name)
        {
            seen.push(name);
        }
    }
    seen.into_iter().map(|n| json!({ "name": n, "path": attach::dir().join(&n).to_string_lossy() })).collect()
}

fn missing(id: u64) -> String {
    format!("No note with id {id}. list_notes gives the ids.")
}

impl mcp::Notes for WinNotes {
    fn list(&mut self, query: Option<&str>, status: Status, limit: usize) -> Value {
        let q = query.map(search::Query::new);
        store(|s| {
            let mut hits: Vec<(i32, &crate::common::store::Note)> = s
                .d
                .notes
                .iter()
                .filter(|n| match status {
                    Status::Active => !n.archived,
                    Status::Archived => n.archived,
                    Status::All => true,
                })
                .filter_map(|n| match &q {
                    Some(q) => q.score(&title_of(&n.text), &rest_of(&n.text)).map(|m| (m.score, n)),
                    None => Some((0, n)),
                })
                .collect();
            hits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.updated.cmp(&a.1.updated)));
            Value::Array(hits.into_iter().take(limit).map(|(_, n)| summary(n)).collect())
        })
    }

    fn read(&mut self, id: u64) -> Result<Value, String> {
        flush_open(id);
        let n = store(|s| s.note(id).cloned()).ok_or_else(|| missing(id))?;
        let w = ui(|u| u.worker);
        let (markdown, files) = if n.rtf.is_empty() {
            (n.text.clone(), Vec::new())
        } else {
            let md = rtf_to_markdown(&n.rtf);
            (md, attachments(w))
        };
        let mut v = summary(&n);
        v["markdown"] = json!(markdown);
        v["created"] = json!(when(n.created));
        v["attachments"] = json!(files);
        v["open"] = json!(note_hwnd(id).is_some());
        Ok(v)
    }

    fn create(&mut self, markdown: &str, color: Option<u8>, pinned: bool) -> Result<Value, String> {
        let w = ui(|u| u.worker);
        load(w, markdown);
        let (text, rtf) = take(w);
        let id = store_mut(|s| {
            let id = s.create(&text, &rtf);
            if let Some(n) = s.note_mut(id) {
                if let Some(c) = color {
                    n.color = c;
                }
                n.pinned = pinned;
            }
            id
        });
        save();
        refresh();
        log::info("mcp: created a note");
        saved_or_err()?;
        Ok(store(|s| s.note(id).map(summary)).unwrap_or(Value::Null))
    }

    fn write(&mut self, id: u64, markdown: &str, append: bool) -> Result<Value, String> {
        flush_open(id);
        let n = store(|s| s.note(id).cloned()).ok_or_else(|| missing(id))?;
        if append && n.text.chars().count() + markdown.chars().count() > crate::common::store::MAX_NOTE_CHARS {
            return Err(crate::common::mcp::too_long());
        }
        let w = ui(|u| u.worker);
        if append {
            // Into the note's own rich text, so its formatting stays as it is.
            if n.rtf.is_empty() {
                rich::load_markdown(w, &n.text);
            } else {
                rich::set_rtf(w, &n.rtf);
            }
            let scratch = Scratch::new();
            load(scratch.0, markdown);
            let extra = rich::get_rtf(scratch.0);
            let end = rich::text(w).len() as i32;
            rich::set_sel(w, end, end);
            if end > 0 {
                rich::insert_text(w, "\r");
            }
            rich::insert_rtf(w, &extra);
        } else {
            load(w, markdown);
        }
        let (text, rtf) = take(w);
        // Before an assistant's change, the note as it was is kept as a version; an open
        // note takes the change as an edit Ctrl+Z takes back (see `note::reload`).
        if rtf != n.rtf {
            crate::win::ui::note::versions::keep(id);
        }
        store_mut(|s| {
            if let Some(n) = s.note_mut(id) {
                n.text = text;
                n.rtf = rtf;
                n.updated = crate::common::util::now();
            }
        });
        save();
        note::reload(id);
        refresh();
        log::info(if append { "mcp: added to a note" } else { "mcp: replaced a note's content" });
        saved_or_err()?;
        Ok(store(|s| s.note(id).map(summary)).unwrap_or(Value::Null))
    }

    fn set_state(&mut self, id: u64, st: State) -> Result<Value, String> {
        let n = store(|s| s.note(id).cloned()).ok_or_else(|| missing(id))?;
        if let Some(a) = st.archived
            && a != n.archived
        {
            set_archived(&[id], a);
        }
        if let Some(p) = st.pinned
            && p != n.pinned
        {
            toggle_pin(id);
        }
        if let Some(c) = st.color {
            set_color(id, c);
        }
        if let Some(d) = st.in_deck
            && d == n.off_deck
        {
            note_command(id, NC_DECK);
        }
        log::info("mcp: changed a note's state");
        saved_or_err()?;
        Ok(store(|s| s.note(id).map(summary)).unwrap_or(Value::Null))
    }
}

#[cfg(test)]
mod connect_tests {

    #[test]
    fn request_lines_are_capped() {
        let big = "x".repeat(MAX_LINE + 10);
        let input = format!("first\r\n{big}\nlast\n");
        let mut r = std::io::BufReader::new(input.as_bytes());
        assert_eq!(read_line(&mut r).unwrap(), Some(Ok("first".to_string())));
        assert_eq!(read_line(&mut r).unwrap(), Some(Err(())), "too long: refused, not kept");
        assert_eq!(read_line(&mut r).unwrap(), Some(Ok("last".to_string())), "and the next line is read as usual");
        assert_eq!(read_line(&mut r).unwrap(), None);
    }

    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("peekaboo-connect-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn json_settings_keep_their_other_entries() {
        let dir = scratch("json");
        let p = dir.join("claude_desktop_config.json");
        // No file yet: made.
        assert_eq!(put_json(&p, "mcpServers", json!({ "command": "a.exe" })), Connect::Done);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["mcpServers"]["peekaboo-notes"]["command"], "a.exe");
        // Other servers and settings stay; ours is replaced; a backup is kept.
        std::fs::write(&p, r#"{ "theme": "dark", "mcpServers": { "other": { "command": "o" }, "peekaboo-notes": { "command": "old" } } }"#).unwrap();
        assert_eq!(put_json(&p, "mcpServers", json!({ "command": "new.exe" })), Connect::Done);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["other"]["command"], "o");
        assert_eq!(v["mcpServers"]["peekaboo-notes"]["command"], "new.exe");
        assert!(std::fs::read_to_string(dir.join("claude_desktop_config.json.bak")).unwrap().contains("old"));
        // A file that isn't JSON is left alone.
        std::fs::write(&p, "not json").unwrap();
        assert_eq!(put_json(&p, "mcpServers", json!({})), Connect::Failed);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "not json");
        // No folder: the app isn't installed.
        assert_eq!(put_json(&dir.join("nowhere").join("x.json"), "mcpServers", json!({})), Connect::Missing);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toml_table_is_added_or_replaced() {
        let dir = scratch("toml");
        let p = dir.join("config.toml");
        let table = "[mcp_servers.peekaboo-notes]\ncommand = 'new.exe'";
        assert_eq!(put_toml(&p, table), Connect::Done);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), format!("{table}\n"));
        std::fs::write(&p, "model = \"x\"\n[mcp_servers.peekaboo-notes]\ncommand = 'old'\n\n[mcp_servers.other]\ncommand = 'o'\n").unwrap();
        assert_eq!(put_toml(&p, table), Connect::Done);
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(s.starts_with("model = \"x\"\n"), "{s}");
        assert!(s.contains("command = 'new.exe'") && !s.contains("'old'"), "{s}");
        assert!(s.contains("[mcp_servers.other]\ncommand = 'o'"), "{s}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
