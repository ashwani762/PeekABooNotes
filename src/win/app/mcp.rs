// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! MCP for assistants, on Windows: a named pipe the `peekaboo-mcp` program connects to,
//! and the note work behind `common::mcp`'s tools. Each client gets a thread that reads
//! its lines; every request is carried out on the UI thread (through `SendMessage`), so
//! an assistant's edits behave like the user's: open notes update, the deck and All Notes
//! refresh, nothing races the store.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::FromRawHandle;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

use serde_json::{Value, json};
use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};

use super::*;
use crate::common::mcp::{self, State, Status};
use crate::common::store::{strip_marker, title_of};
use crate::common::{attach, search};

/// Sent to the main window: wParam points at the request line, lParam at the reply.
pub const WM_MCP: u32 = WM_APP + 77;

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
        if !LISTENING.swap(true, Ordering::SeqCst) {
            let main = MAIN.load(Ordering::SeqCst);
            let _ = std::thread::Builder::new().name("mcp".into()).spawn(move || serve(main));
            log::info("mcp: on");
        }
    } else if LISTENING.load(Ordering::SeqCst) {
        // The server waits for a connection: make one, so it sees it's off and stops.
        let _ = std::fs::OpenOptions::new().read(true).write(true).open(pipe_name());
        log::info("mcp: off");
    }
}

/// Where `peekaboo-mcp.exe` is (beside the app).
pub fn bridge_path() -> std::path::PathBuf {
    crate::win::util::exe_path().with_file_name("peekaboo-mcp.exe")
}

/// Assistant apps that run local MCP servers, as the Settings menu lists them.
pub const CLIENTS: [&str; 9] = ["ChatGPT (desktop app)", "Claude Desktop", "Claude Code", "Gemini CLI", "Codex", "Cursor", "VS Code", "Windsurf", "LM Studio"];

/// What to paste for assistant app `i` of `CLIENTS`: a command to run, or a piece of its
/// MCP settings file.
pub fn setup_for(i: usize) -> String {
    // From the Store/MSIX package the bridge is a command on PATH (an execution alias);
    // its real folder changes with every update.
    let exe = if packaged() { "peekaboo-mcp.exe".to_string() } else { bridge_path().to_string_lossy().into_owned() };
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

fn serve(main: isize) {
    let name = wide(&pipe_name());
    loop {
        let h = unsafe {
            CreateNamedPipeW(
                name.as_ptr(),
                PIPE_ACCESS_DUPLEX,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                64 * 1024,
                64 * 1024,
                0,
                std::ptr::null(),
            )
        };
        if h == INVALID_HANDLE_VALUE {
            log::warn(&format!("mcp: couldn't open the pipe ({})", unsafe { GetLastError() }));
            return;
        }
        let ok = unsafe { ConnectNamedPipe(h, std::ptr::null_mut()) } != 0 || unsafe { GetLastError() } == ERROR_PIPE_CONNECTED;
        let file = unsafe { File::from_raw_handle(h as _) };
        if !ENABLED.load(Ordering::SeqCst) {
            LISTENING.store(false, Ordering::SeqCst);
            return;
        }
        if ok {
            let _ = std::thread::Builder::new().name("mcp client".into()).spawn(move || client(main, file));
        }
    }
}

fn client(main: isize, file: File) {
    log::info("mcp: an assistant connected");
    let mut out = match file.try_clone() {
        Ok(f) => f,
        Err(_) => return,
    };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if !ENABLED.load(Ordering::SeqCst) {
            break;
        }
        if line.trim().is_empty() {
            continue;
        }
        let mut reply = String::new();
        unsafe { SendMessageW(main as HWND, WM_MCP, &line as *const String as usize, &mut reply as *mut String as isize) };
        // One line back for every line in; empty for a notification.
        if out.write_all(reply.as_bytes()).and_then(|_| out.write_all(b"\n")).and_then(|_| out.flush()).is_err() {
            break;
        }
    }
    log::info("mcp: the assistant disconnected");
}

/// `WM_MCP`, on the UI thread.
pub fn on_message(wp: WPARAM, lp: LPARAM) {
    let (line, reply) = unsafe { (&*(wp as *const String), &mut *(lp as *mut String)) };
    *reply = mcp::handle(line, &mut WinNotes).unwrap_or_default();
}

// ---------------------------------------------------------------- the work

struct WinNotes;

fn when(ts: i64) -> String {
    let t = crate::os::local_time(ts);
    format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year, t.month, t.day, t.hour, t.minute)
}

/// Everything after the title line, list markers aside (what search looks in).
fn rest_of(text: &str) -> String {
    text.lines().skip_while(|l| strip_marker(l).trim().is_empty()).skip(1).map(strip_marker).collect::<Vec<_>>().join("\n")
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
        Ok(store(|s| s.note(id).map(summary)).unwrap_or(Value::Null))
    }
}
