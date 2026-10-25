// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The Model Context Protocol (MCP) side of the app: lets an AI assistant read, find
//! and write notes. Messages are JSON-RPC 2.0, one per line (the `peekaboo-mcp` program
//! carries them between the assistant and the running app). This part is portable: it
//! speaks the protocol and checks arguments; each platform supplies a `Notes` that does
//! the work. Nothing here deletes a note: archiving is as far as it goes.

use serde_json::{Value, json};

/// The protocol version spoken when the client doesn't name one we know.
const PROTOCOL: &str = "2025-06-18";

/// Which notes a listing covers.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Status {
    Active,
    Archived,
    All,
}

/// Changes to a note's state; None leaves that part as it is.
#[derive(Default, Debug, PartialEq)]
pub struct State {
    pub archived: Option<bool>,
    pub pinned: Option<bool>,
    pub color: Option<u8>,
    pub in_deck: Option<bool>,
}

/// What a platform does for the tools. Results are JSON for the assistant; errors are
/// sentences for it to read.
pub trait Notes {
    fn list(&mut self, query: Option<&str>, status: Status, limit: usize) -> Value;
    fn read(&mut self, id: u64) -> Result<Value, String>;
    fn create(&mut self, markdown: &str, color: Option<u8>, pinned: bool) -> Result<Value, String>;
    /// Replace the content, or add to the end of it.
    fn write(&mut self, id: u64, markdown: &str, append: bool) -> Result<Value, String>;
    fn set_state(&mut self, id: u64, state: State) -> Result<Value, String>;
}

/// The colour names the tools take and give, by colour index.
pub fn color_name(i: u8) -> &'static str {
    crate::common::theme::color(i).name
}

pub fn color_of(name: &str) -> Option<u8> {
    (0..crate::common::theme::COLORS.len() as u8).find(|&i| color_name(i).eq_ignore_ascii_case(name.trim()))
}

fn color_names() -> Vec<&'static str> {
    crate::common::theme::ORDER.iter().map(|&i| color_name(i)).collect()
}

/// The tools, as JSON text (as data rather than `json!` code, which compiles to many KB).
/// "__COLORS__" stands for the list of colour names.
const TOOLS: &str = r#"[
 {
  "name": "list_notes",
  "title": "List or search notes",
  "description": "Lists notes, newest first, with each one's id, title, first line, colour and state. With a query it searches instead (forgiving: words in any order, word starts, small typos), best matches first.",
  "inputSchema": {
   "type": "object",
   "properties": {
    "query": {
     "type": "string",
     "description": "Words to look for. Leave out to list."
    },
    "status": {
     "type": "string",
     "enum": [
      "active",
      "archived",
      "all"
     ],
     "description": "Which notes: active (the default), archived, or all."
    },
    "limit": {
     "type": "integer",
     "minimum": 1,
     "maximum": 500,
     "description": "Most notes to return (default 50)."
    }
   }
  },
  "annotations": {
   "readOnlyHint": true
  }
 },
 {
  "name": "read_note",
  "title": "Read a note",
  "description": "A note's full content as Markdown (the first line is its title), with its colour, state, dates and attached files (names and paths on disk).",
  "inputSchema": {
   "type": "object",
   "properties": {
    "id": {
     "type": "integer",
     "description": "The note's id, from list_notes."
    }
   },
   "required": [
    "id"
   ]
  },
  "annotations": {
   "readOnlyHint": true
  }
 },
 {
  "name": "create_note",
  "title": "Create a note",
  "description": "Creates a note from Markdown. The first line is its title. Supported: **bold**, _italic_, ~~strike~~, <u>underline</u>, - bullets, 1. numbers, - [ ] / - [x] checklist items, > quotes (lists inside them as > - item; indented two spaces under a list item), ``` code blocks ```, | tables | with a | --- | row (:---: centres a column), <p align=\"center\">centred</p> or align=\"right\" lines, [links](https://...), and files from the user's Pictures, Documents, Downloads, Desktop, Videos, Music or the temp folder as ![name](C:/path/picture.png) or [name](C:/path/file.pdf), which are copied in as attachments (links to files elsewhere stay links). Other headings come in as plain text.",
  "inputSchema": {
   "type": "object",
   "properties": {
    "markdown": {
     "type": "string",
     "description": "The note. The first line becomes the title."
    },
    "color": {
     "type": "string",
     "enum": "__COLORS__",
     "description": "Paper colour. Leave out for the user's default."
    },
    "pinned": {
     "type": "boolean",
     "description": "Keep it open on top of other windows when opened."
    }
   },
   "required": [
    "markdown"
   ]
  },
  "annotations": {
   "readOnlyHint": false,
   "destructiveHint": false
  }
 },
 {
  "name": "update_note",
  "title": "Replace a note's content",
  "description": "Replaces a note's whole content with this Markdown (same format as create_note). Read the note first and keep what should stay: what isn't in the new Markdown is gone.",
  "inputSchema": {
   "type": "object",
   "properties": {
    "id": {
     "type": "integer"
    },
    "markdown": {
     "type": "string"
    }
   },
   "required": [
    "id",
    "markdown"
   ]
  },
  "annotations": {
   "readOnlyHint": false,
   "destructiveHint": true
  }
 },
 {
  "name": "append_to_note",
  "title": "Add to a note",
  "description": "Adds Markdown to the end of a note, keeping everything already in it (formatting included).",
  "inputSchema": {
   "type": "object",
   "properties": {
    "id": {
     "type": "integer"
    },
    "markdown": {
     "type": "string"
    }
   },
   "required": [
    "id",
    "markdown"
   ]
  },
  "annotations": {
   "readOnlyHint": false,
   "destructiveHint": false
  }
 },
 {
  "name": "set_note_state",
  "title": "Archive, restore, pin or colour a note",
  "description": "Changes a note's state: archive it (or bring it back), pin it, change its colour, or take it out of (or put it back in) the deck at the screen edge. Notes are never deleted from here; archiving puts one away and can be undone.",
  "inputSchema": {
   "type": "object",
   "properties": {
    "id": {
     "type": "integer"
    },
    "archived": {
     "type": "boolean"
    },
    "pinned": {
     "type": "boolean"
    },
    "color": {
     "type": "string",
     "enum": "__COLORS__"
    },
    "in_deck": {
     "type": "boolean"
    }
   },
   "required": [
    "id"
   ]
  },
  "annotations": {
   "readOnlyHint": false,
   "destructiveHint": false,
   "idempotentHint": true
  }
 }
]"#;

fn tools() -> Value {
    let colors = serde_json::to_string(&color_names()).unwrap_or_else(|_| "[]".into());
    serde_json::from_str(&TOOLS.replace("\"__COLORS__\"", &colors)).unwrap_or(Value::Null)
}

fn id_arg(args: &Value) -> Result<u64, String> {
    args.get("id").and_then(Value::as_u64).ok_or_else(|| "Give the note's id (a number from list_notes).".to_string())
}

fn text_arg<'a>(args: &'a Value, name: &str) -> Result<&'a str, String> {
    let s = args.get(name).and_then(Value::as_str).ok_or_else(|| format!("Give \"{name}\" as text."))?;
    if s.chars().count() > crate::common::store::MAX_NOTE_CHARS {
        return Err(too_long());
    }
    Ok(s)
}

/// Why a note can't take more.
pub fn too_long() -> String {
    format!("Too long: a note holds up to {} characters.", crate::common::store::MAX_NOTE_CHARS)
}

fn color_arg(args: &Value) -> Result<Option<u8>, String> {
    match args.get("color").and_then(Value::as_str) {
        None => Ok(None),
        Some(name) => color_of(name).map(Some).ok_or_else(|| format!("No colour called \"{name}\". Colours: {}.", color_names().join(", "))),
    }
}

fn call(notes: &mut dyn Notes, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        "list_notes" => {
            let status = match args.get("status").and_then(Value::as_str).unwrap_or("active") {
                "active" => Status::Active,
                "archived" => Status::Archived,
                "all" => Status::All,
                other => return Err(format!("No status \"{other}\": use \"active\", \"archived\" or \"all\".")),
            };
            let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(50).clamp(1, 500) as usize;
            let query = args.get("query").and_then(Value::as_str).filter(|q| !q.trim().is_empty());
            Ok(notes.list(query, status, limit))
        }
        "read_note" => notes.read(id_arg(args)?),
        "create_note" => {
            let md = text_arg(args, "markdown")?;
            if md.trim().is_empty() {
                return Err("The note would be empty.".into());
            }
            notes.create(md, color_arg(args)?, args.get("pinned").and_then(Value::as_bool).unwrap_or(false))
        }
        "update_note" => notes.write(id_arg(args)?, text_arg(args, "markdown")?, false),
        "append_to_note" => notes.write(id_arg(args)?, text_arg(args, "markdown")?, true),
        "set_note_state" => {
            let b = |k: &str| args.get(k).and_then(Value::as_bool);
            let state = State { archived: b("archived"), pinned: b("pinned"), color: color_arg(args)?, in_deck: b("in_deck") };
            if state == State::default() {
                return Err("Nothing to change: give archived, pinned, color or in_deck.".into());
            }
            notes.set_state(id_arg(args)?, state)
        }
        _ => Err(format!("No tool called \"{name}\".")),
    }
}

fn reply(id: &Value, result: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

fn error(id: &Value, code: i64, message: &str) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }).to_string()
}

/// Answer one JSON-RPC message. None for notifications, which get no answer.
/// The answer to a request line too long to take.
pub fn request_too_long() -> String {
    error(&Value::Null, -32600, "The request is too long.")
}

pub fn handle(line: &str, notes: &mut dyn Notes) -> Option<String> {
    // Some clients (a PowerShell pipe, for one) start the first line with a byte order mark.
    let line = line.trim_start_matches('\u{FEFF}');
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error(&Value::Null, -32700, "Not JSON.")),
    };
    // Batches (an array of requests) aren't taken: say so rather than leave the client
    // waiting for an answer.
    if !msg.is_object() {
        return Some(error(&Value::Null, -32600, "Send one request per line, as a JSON object."));
    }
    let id = msg.get("id").cloned()?; // no id: a notification
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(Value::as_str);
            let version = match asked {
                Some(v @ ("2024-11-05" | "2025-03-26" | "2025-06-18")) => v,
                _ => PROTOCOL,
            };
            reply(
                &id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "peekaboo-notes", "title": "PeekABoo Notes", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": "The user's sticky notes. Find notes with list_notes (it searches too), read one with read_note, and write with create_note, update_note or append_to_note. Notes are Markdown; the first line is the title. Nothing can be deleted from here: set_note_state can archive a note, which the user can undo."
                }),
            )
        }
        "ping" => reply(&id, json!({})),
        "tools/list" => reply(&id, json!({ "tools": tools() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let result = match call(notes, name, &args) {
                Ok(v) => {
                    let text = serde_json::to_string_pretty(&v).unwrap_or_default();
                    // Structured content is an object; lists go in one.
                    let structured = if v.is_object() { v } else { json!({ "notes": v }) };
                    json!({ "content": [{ "type": "text", "text": text }], "structuredContent": structured, "isError": false })
                }
                Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
            };
            reply(&id, result)
        }
        _ => error(&id, -32601, &format!("Unknown method \"{method}\".")),
    })
}

#[cfg(test)]
mod tests {

    #[test]
    fn batches_and_unknown_statuses_get_errors() {
        struct None_;
        impl Notes for None_ {
            fn list(&mut self, _: Option<&str>, _: Status, _: usize) -> Value {
                json!([])
            }
            fn read(&mut self, _: u64) -> Result<Value, String> {
                Err("no".into())
            }
            fn create(&mut self, _: &str, _: Option<u8>, _: bool) -> Result<Value, String> {
                Err("no".into())
            }
            fn write(&mut self, _: u64, _: &str, _: bool) -> Result<Value, String> {
                Err("no".into())
            }
            fn set_state(&mut self, _: u64, _: State) -> Result<Value, String> {
                Err("no".into())
            }
        }
        let batch = handle(r#"[{"jsonrpc":"2.0","id":1,"method":"tools/list"}]"#, &mut None_).unwrap();
        assert!(batch.contains("-32600"), "{batch}");
        let bad = handle(r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_notes","arguments":{"status":"bogus"}}}"#, &mut None_).unwrap();
        assert!(bad.contains("No status") && bad.contains("\"isError\":true"), "{bad}");
    }

    use super::*;

    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
    }

    impl Notes for Fake {
        fn list(&mut self, query: Option<&str>, status: Status, limit: usize) -> Value {
            self.calls.push(format!("list {query:?} {status:?} {limit}"));
            json!([{ "id": 1, "title": "Groceries" }])
        }
        fn read(&mut self, id: u64) -> Result<Value, String> {
            if id == 1 { Ok(json!({ "id": 1, "markdown": "Groceries\n- milk" })) } else { Err("No note with id 2.".into()) }
        }
        fn create(&mut self, markdown: &str, color: Option<u8>, pinned: bool) -> Result<Value, String> {
            self.calls.push(format!("create {markdown:?} {color:?} {pinned}"));
            Ok(json!({ "id": 9 }))
        }
        fn write(&mut self, id: u64, markdown: &str, append: bool) -> Result<Value, String> {
            self.calls.push(format!("write {id} {markdown:?} {append}"));
            Ok(json!({ "id": id }))
        }
        fn set_state(&mut self, id: u64, state: State) -> Result<Value, String> {
            self.calls.push(format!("state {id} {state:?}"));
            Ok(json!({ "id": id }))
        }
    }

    fn ask(f: &mut Fake, msg: Value) -> Value {
        serde_json::from_str(&handle(&msg.to_string(), f).unwrap()).unwrap()
    }

    fn call_tool(f: &mut Fake, name: &str, args: Value) -> Value {
        ask(f, json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": args } }))["result"].clone()
    }

    #[test]
    fn speaks_the_protocol() {
        let mut f = Fake::default();
        let init = ask(&mut f, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-03-26" } }));
        assert_eq!(init["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(init["result"]["serverInfo"]["name"], "peekaboo-notes");
        assert_eq!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(), &mut f), None);
        let list = ask(&mut f, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let names: Vec<&str> = list["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["list_notes", "read_note", "create_note", "update_note", "append_to_note", "set_note_state"]);
        assert!(!names.iter().any(|n| n.contains("delete")));
        assert_eq!(ask(&mut f, json!({ "jsonrpc": "2.0", "id": 3, "method": "nope" }))["error"]["code"], -32601);
        assert_eq!(handle("{oops", &mut f).map(|r| r.contains("-32700")), Some(true));
        let bom = format!("\u{FEFF}{}", json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }));
        assert!(handle(&bom, &mut f).is_some_and(|r| r.contains("list_notes")));
    }

    #[test]
    fn calls_tools() {
        let mut f = Fake::default();
        let r = call_tool(&mut f, "list_notes", json!({ "query": "milk", "status": "all" }));
        assert_eq!(r["isError"], false);
        assert_eq!(r["structuredContent"]["notes"][0]["title"], "Groceries");
        assert_eq!(call_tool(&mut f, "read_note", json!({ "id": 2 }))["isError"], true);
        call_tool(&mut f, "create_note", json!({ "markdown": "Trip\n- passport", "color": "mint", "pinned": true }));
        call_tool(&mut f, "append_to_note", json!({ "id": 4, "markdown": "- charger" }));
        call_tool(&mut f, "set_note_state", json!({ "id": 4, "archived": true }));
        assert_eq!(call_tool(&mut f, "create_note", json!({ "markdown": "x", "color": "plaid" }))["isError"], true);
        assert_eq!(call_tool(&mut f, "set_note_state", json!({ "id": 4 }))["isError"], true);
        assert_eq!(
            f.calls,
            [
                "list Some(\"milk\") All 50",
                &format!("create \"Trip\\n- passport\" Some({}) true", color_of("Mint").unwrap()),
                "write 4 \"- charger\" true",
                "state 4 State { archived: Some(true), pinned: None, color: None, in_deck: None }",
            ]
        );
    }
}
