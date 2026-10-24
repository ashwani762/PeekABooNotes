# PeekABoo Notes for developers

How to build it, how it's put together, and the details behind the features. For what it does, see the [README](README.md).

## Build and run

Needs Rust (edition 2024) and Windows 10 or 11.

```
cargo build --release
target\release\peekaboo-notes.exe
```

`cargo test` runs the unit tests: the note store, Markdown in and out of the editor (lists, emphasis, links, emoji, attachments), search, the zip writer, the MCP protocol, the string files and plural rules, and a check that `src/common` stays free of OS calls.

### Installers

- **Setup wizard (Inno Setup):** `powershell -ExecutionPolicy Bypass -File packaging\inno\build-setup.ps1` builds `target\setup\PeekABooNotes-Setup-<version>.exe` (add `-NoBuild` to reuse `target\release`). It installs per user without admin, offers a desktop shortcut and starting with Windows, and its uninstaller asks whether to delete the notes too. Needs Inno Setup 6 (`winget install JRSoftware.InnoSetup`). Install silently with `/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`.
- **MSIX:** `powershell -ExecutionPolicy Bypass -File packaging\build-msix.ps1` builds a package in `target\msix\` (pass `-CertPath`/`-Password` or `-Thumbprint` to sign it). An installed copy starts with Windows through the package's startup task, and uninstalling it from Settings > Apps leaves the notes in Documents.

Both take the version from `Cargo.toml`.

### Developer switches

- `PEEKABOO_THEME=light|dark` forces an appearance.
- `PEEKABOO_PERF=1` logs frame times, and which parts asked for the frames, to `%TEMP%\peekaboo-perf.txt`.
- `PEEKABOO_TRACE=1` adds DEBUG lines (menu events and the like) to `logs\peekaboo.log`, and symbolized stacks to panic reports.
- `PEEKABOO_CRASH=panic|fault|overflow` crashes on purpose right after startup, to test crash reporting.

A crash writes a minidump to `logs\crash-YYYYMMDD-HHMMSS.dmp` (the newest three are kept), and its report, with raw `module+offset` stack addresses, to the log and to `crash.log` in the notes folder. Open the dump in Visual Studio or WinDbg next to the matching build's `.pdb`.

## Code map

The app is split so other platforms can be added beside Windows: `src/common/` is portable and never calls an operating system directly (a test enforces it), and each platform module (today `src/win/`) supplies the windows, text, editor and system services, plus the handful of functions in `crate::os` that `common` relies on (local time, the Documents folder, preferred languages, locale date formats, a system description).

| Path | Contents |
|---|---|
| `src/main.rs` | Entry point: picks the platform module |
| `src/common/` | The portable app: the note store (`store.rs`), interface strings (`i18n/`), search (`search.rs`), attachments (`attach.rs`), emoji (`emoji.rs`), the MCP protocol and tools (`mcp.rs`), the zip writer (`zip.rs`), design tokens (`theme.rs`), springs (`anim.rs`), the log (`log.rs`) and small helpers (`util.rs`) |
| `src/win/` | Windows: `os.rs` (what `common` asks of the OS), `app/` (message loop, note lifecycle, menus, import, export and share, autostart, the MCP pipe), `ui/` (deck, note windows, All Notes, Settings, menus, toasts, tooltips, flying cards), `editor/` (RichEdit, links, quotes, code blocks and tables, and the checklist, colour-emoji and header-fade overlays), `render/` (software canvas, DirectWrite, frame clock), `crash.rs`, `theme.rs`, `util.rs` |
| `src/bin/peekaboo-mcp.rs` | The MCP bridge: stdin/stdout to the app's pipe |

### The editor

Notes are Windows' RichEdit (Msftedit), with a few things layered on:

- **Lists and checklists** are paragraph formats; a checklist item starts with a ☐/☑ placeholder that the checklist overlay draws as a circle.
- **Colour emoji** are drawn over RichEdit's monochrome glyphs by an overlay. Emoji variation selectors (U+FE0F) are kept out of the control as hidden private-use stand-ins, because RichEdit stops drawing list bullets once one is in a document.
- **Links** are RichEdit's named links (a hidden `HYPERLINK "url"` field before the shown text). RichEdit's own address spotting is off, because it picks those links' colour itself (white on paper dimmed for dark mode); typed and pasted addresses become named links when a space, Tab or Enter ends them, or the note opens.
- **Quotes, code blocks and tables** are RichEdit table rows (`rich/blocks.rs`), so RichEdit draws, edits and stores them: a quote is one cell with a bar down its left border, a code block one shaded cell in Cascadia Mono (Consolas without it), a table any number of cells in a light grid with a tinted first row. Which is which is the cell margin it was made with (`\trgaph` in the RTF); borders a block doesn't show are drawn in the colour around it, since RichEdit outlines borderless cells in grey. Colours follow the note and the theme, and cells are refitted to the note's width when it's resized. A quote's paragraphs keep their own kind (bullets, numbers, checklist items), and a quote can step in under a list item (Tab, Shift+Tab; the row's left indent). RichEdit keeps an empty paragraph between two blocks; Markdown export skips it. Several-step changes (a new block, a checkbox) go through `rich::still`, which holds the view still and lets one Ctrl+Z take the whole change back; restyling blocks (colours, refitting to the width) is kept out of the undo history.
- **Undo** holds 110 steps in an open editor. Past those, Ctrl+Z goes on through the note's last 20 saved versions (`note/history.rs`, in memory for the session, at most 2 MB a note), so a note closed and reopened, left in All Notes for another, or changed by an assistant can still go back; redo comes forward again.
- **Alignment** (left, centre, right) is RichEdit's paragraph alignment; a picture is a paragraph of its own, so it aligns the same way.
- **Bulk changes** (loading a note, applying a theme) freeze layout, drawing, notifications and undo until they're done, so a long note is laid out once instead of once per paragraph.
- The overlays look only at the text on screen, so they cost the same in a long note as in a short one.

### Limits

A note holds up to 100,000 characters (`MAX_NOTE_CHARS` in `store.rs`): typing and pasting stop there and the footer says the note is full; import skips longer files and assistants are told the text is too long. A note saved longer (from an older version) opens whole and just can't grow.

Measured on the release build with 2,003 notes in a 2.4 MB `notes.json`: start to the deck in 0.16 s, about 5 MB of memory at rest, All Notes open in under a second, a 3.6 MB save in 6 ms.

## Your data

Notes live in one readable file, `Documents\PeekABoo Notes\notes.json`:

- each note keeps its RTF and a plain-text Markdown copy (for search, previews and assistants);
- writes are atomic (temp file, then rename);
- `notes.backup.json` holds the last good copy from the previous launch. If `notes.json` can't be read (locked by another program, or damaged), the app opens the backup instead and never writes over the original;
- attachments are copied into `attachments\` beside it and linked by name.

There is no network access and no telemetry.

`logs\peekaboo.log` in the same folder records what happened (start and exit, load and save errors, import and export counts), never what the notes say. It rotates at 512 KB, keeping one older file (`peekaboo.1.log`).

**Portable mode:** an empty file named `portable` next to `peekaboo-notes.exe` keeps the notes in `PeekABooData\` beside the exe instead.

## Import, export and share

- **Export** writes one Markdown file per note into a folder you pick (the picker remembers it). It never overwrites a file; a name already in the folder gets "(2)".
- **Import** reads `.md` and `.txt` files and turns their Markdown into real formatting: `**bold**`, `_italic_`, `~~strike~~`, `<u>underline</u>`, bullets, numbers, `[ ]` / `[x]` checklists, nested lists, `[links](https://...)`, `> ` quotes (with lists inside, `> - item`, and under list items, `  > text`), fenced code blocks, pipe tables (`:---:` and `---:` align a column) and `<p align="center">` / `"right"` lines. Headings below the title come in as plain text. Plain text that only looks like a marker is written escaped (`\- not a list`) and comes back as text.
- **Attachments** go along: export copies them into an `attachments` folder beside the Markdown and links them relatively (`![photo.jpg](attachments/photo.jpg)`); import copies linked local files back in.
- **Readable anywhere:** plain UTF-8; lines end in Markdown's line break (two spaces) so viewers keep them apart as the note has them; an address that links to itself is written as just the address.
- **Share** writes the same files for the selected notes into one zip (stored, not compressed) under `%TEMP%\PeekABoo Share` and hands it to the Windows share sheet; without one, Explorer shows the zip.

A round trip keeps nesting, emphasis, links, attachments and ticked items intact.

## AI assistants (MCP)

Off until **Settings > Data > Allow assistants (MCP)**. **Copy Setup** puts the right snippet on the clipboard:

| App | Where it goes |
|---|---|
| ChatGPT (desktop app) | Add an MCP server of type STDIO, with the copied command |
| Claude Desktop | `claude_desktop_config.json` (the copied `mcpServers` block) |
| Claude Code | Run the copied `claude mcp add ...` command |
| Gemini CLI | `~/.gemini/settings.json` |
| Codex | `~/.codex/config.toml` (the copied `[mcp_servers...]` table) |
| Cursor, Windsurf, LM Studio | Their `mcp.json` (the copied `mcpServers` block) |
| VS Code (Copilot) | `.vscode/mcp.json` (the copied `servers` block) |

Any other app that runs local MCP servers works the same way: its command is `peekaboo-mcp.exe` from the app's folder (from the MSIX, just `peekaboo-mcp.exe`). ChatGPT on the web only reaches servers on the internet, so it can't use this one.

The tools: `list_notes` (lists, or searches forgivingly), `read_note` (Markdown, colour, dates, attachments), `create_note`, `update_note`, `append_to_note` (keeps the note's formatting), and `set_note_state` (archive or restore, pin, colour, deck). There is no delete. Local files linked in an assistant's Markdown (`![chart](C:/path/chart.png)`) are copied in as attachments.

How it works: the assistant starts `peekaboo-mcp.exe`, which carries its JSON-RPC messages over the named pipe `\\.\pipe\PeekABooNotes.mcp.<user>` to the running app (starting it if needed). The app answers on its UI thread, so open notes, the deck and All Notes update as the assistant writes. The pipe only takes connections from this computer and this user, and only while the switch is on.

## Shortcuts

**Anywhere** (if another app already uses one, PeekABoo Notes goes without it)

| Keys | Action |
|---|---|
| Ctrl+Alt+N | New note |
| Ctrl+Alt+A | All Notes |
| Ctrl+Alt+L | Archive |
| Ctrl+Alt+O | Reopen last note |
| Ctrl+Alt+H | Show or hide the deck |
| Ctrl+Alt+E | Move the deck to the other edge |

**In a note**

| Keys | Action |
|---|---|
| Ctrl+N | New note |
| Esc / Ctrl+W | Close |
| Ctrl+B / I / U | Bold / italic / underline |
| Ctrl+Shift+X | Strikethrough |
| Ctrl+Shift+8 / 7 | Bulleted / numbered list |
| Ctrl+Shift+L | Checklist |
| Ctrl+Enter | Tick or untick the checklist item |
| Tab / Shift+Tab | Indent / outdent a list item; in a table, next / previous cell (Tab in the last cell adds a row); in a code block, indent / outdent the line |
| Ctrl+L / E / R | Align left / centre / right |
| Ctrl+' | Quote |
| Ctrl+Shift+M | Code block |
| Ctrl+Shift+T | Table |
| Ctrl+K | Link the selected text, or change the link at the caret |
| Ctrl+. | Next colour |
| Ctrl+Shift+P | Keep on top |
| Ctrl+Shift+A | Archive |
| Ctrl+Shift+Del | Delete (with Undo) |
| Ctrl+[ / Ctrl+] | Previous / next note |

**In All Notes:** Ctrl+F searches; Ctrl+Shift+A archives or restores and Ctrl+Shift+Del deletes the selected notes; Ctrl+Tab leaves the note being edited. Tab and Shift+Tab move through All Notes and Settings with a focus ring; Space or Enter presses, the arrow keys move along segments and swatches. In menus, the arrow keys reach every row, colour and format button. Paste is always plain text, so notes keep a consistent look.

## Languages

English, German, Spanish, French, Hindi and Japanese are built in. A new install starts in English; Settings › General › Language picks another or **Same as Windows** (Windows' preferred display languages: the exact tag, then the base language, then English). Dates and times are formatted by Windows for the chosen locale.

Interface text lives in Android-style resource files, one per language: `src/common/i18n/strings/strings.<tag>.xml`.

```xml
<resources>
    <string name="menu_new_note">New Note</string>
    <string name="window_all_notes">All Notes — %1$s</string>
    <plurals name="notes_count">
        <item quantity="one">%d note</item>
        <item quantity="other">%d notes</item>
    </plurals>
</resources>
```

- **Placeholders:** `%s` and `%d` in order, or `%1$s` and `%2$d` by position.
- **Plurals:** the CLDR categories (zero, one, two, few, many, other).
- **Fallback:** English is the reference; any key a translation lacks falls back to it.
- Buttons and menu items never end in "…".

**Adding a language**
1. Copy `strings.en.xml` to `strings.<tag>.xml` (for example `strings.it.xml` or `strings.pt-BR.xml`) and translate the text.
2. Either add the file to `BUILTIN` in `src/common/i18n/mod.rs`, or drop it into a `lang` folder next to `peekaboo-notes.exe` (or in the notes folder).

Files in `lang` are read at startup and override the built-in strings, so a translation can be tried without rebuilding. Every build copies the built-in files to `target\<profile>\lang\`. `cargo test` checks that every built-in language has exactly the keys and placeholders of English.

## License

Copyright (C) 2026 Ashwani Kumar. GPL-3.0-or-later; see [LICENSE](LICENSE). Every source file carries `SPDX-License-Identifier: GPL-3.0-or-later`.
