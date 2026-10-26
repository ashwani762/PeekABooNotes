// Copyright (C) 2026 Ashwani Kumar
// SPDX-License-Identifier: GPL-3.0-or-later

//! The assistants' (MCP) pipe: its name, and who is at either end. Shared by the app
//! (`app/mcp.rs`) and the bridge (`bin/peekaboo-mcp.rs`, which takes this file in by its
//! path), so it uses nothing but the standard library and Windows.
//!
//! Another user on the same computer must never get between the two: the name carries the
//! user's SID (not their user name, which another account can share or guess), the app
//! makes the pipe's first instance only if no one holds the name already
//! (`FILE_FLAG_FIRST_PIPE_INSTANCE`) and lets only its own user open it, and the bridge
//! talks only to a server running as its own user, which can identify it but not act as it.

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};

/// The user `process` runs as, as a SID in text ("S-1-5-21-...").
fn process_sid(process: HANDLE) -> Option<String> {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }
        let mut len = 0u32;
        GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut len);
        // 8-byte words, so the TOKEN_USER in it is aligned.
        let mut buf = vec![0u64; (len as usize).div_ceil(8).max(8)];
        let got = GetTokenInformation(token, TokenUser, buf.as_mut_ptr() as _, (buf.len() * 8) as u32, &mut len) != 0;
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

/// The running user's SID as text.
pub fn user_sid() -> Option<String> {
    process_sid(unsafe { GetCurrentProcess() })
}

/// The pipe of user `sid`.
fn name_for(sid: &str) -> String {
    format!(r"\\.\pipe\PeekABooNotes.mcp.{sid}")
}

/// The running user's pipe; None when the user can't be told.
pub fn pipe_name() -> Option<String> {
    user_sid().map(|s| name_for(&s))
}

/// Whether the process at the server end of pipe `client` (a handle to the client's end)
/// runs as the running user.
#[cfg_attr(not(test), allow(dead_code))] // the bridge's check; the app only serves
pub fn served_by_me(client: HANDLE) -> bool {
    unsafe {
        let mut pid = 0u32;
        if GetNamedPipeServerProcessId(client, &mut pid) == 0 {
            return false;
        }
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if p.is_null() {
            return false;
        }
        let sid = process_sid(p);
        CloseHandle(p);
        sid.is_some() && sid == user_sid()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, PIPE_ACCESS_DUPLEX, SECURITY_IDENTIFICATION};
    use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES};

    #[test]
    fn the_name_is_the_users_sid() {
        let sid = user_sid().unwrap();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        assert_eq!(pipe_name().unwrap(), format!(r"\\.\pipe\PeekABooNotes.mcp.{sid}"));
        assert!(!pipe_name().unwrap().contains(&std::env::var("USERNAME").unwrap_or_else(|_| "\u{0}".into())));
    }

    /// On a pipe of the test's own (never the app's): a name already held can't be taken
    /// as a first instance, and the client sees its server is its own user.
    #[test]
    fn a_held_name_isnt_taken_and_the_server_is_checked() {
        let name: Vec<u16> = format!(r"\\.\pipe\PeekABooNotes.test.{}.{}", std::process::id(), user_sid().unwrap()).encode_utf16().chain(Some(0)).collect();
        let make = |flags| unsafe { CreateNamedPipeW(name.as_ptr(), PIPE_ACCESS_DUPLEX | flags, PIPE_TYPE_BYTE | PIPE_REJECT_REMOTE_CLIENTS, PIPE_UNLIMITED_INSTANCES, 4096, 4096, 0, std::ptr::null()) };
        let first = make(FILE_FLAG_FIRST_PIPE_INSTANCE);
        assert_ne!(first, INVALID_HANDLE_VALUE);
        // Held: a second "first" instance is refused (as a squatter's name refuses the app's).
        let again = make(FILE_FLAG_FIRST_PIPE_INSTANCE);
        assert_eq!(again, INVALID_HANDLE_VALUE);
        let path = String::from_utf16_lossy(&name[..name.len() - 1]);
        let client = std::fs::OpenOptions::new().read(true).write(true).security_qos_flags(SECURITY_IDENTIFICATION).open(&path).unwrap();
        assert!(served_by_me(client.as_raw_handle() as HANDLE));
        drop(client);
        unsafe { CloseHandle(first) };
        // Not a pipe at all: no server to vouch for.
        let file = std::env::temp_dir().join(format!("peekaboo-pipe-test-{}", std::process::id()));
        let f = std::fs::File::create(&file).unwrap();
        assert!(!served_by_me(f.as_raw_handle() as HANDLE));
        drop(f);
        let _ = std::fs::remove_file(&file);
    }
}
