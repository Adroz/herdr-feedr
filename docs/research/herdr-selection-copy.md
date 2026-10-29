# Research: how herdr does text selection and copy

Ticket: [#15](https://github.com/Adroz/herdr-feedr/issues/15). Investigated against the herdr source, tag **v0.9.3** (`7b116c05`, the version installed locally: `herdr --version` gives 0.9.3), cloned from [herdrdev/herdr](https://github.com/herdrdev/herdr). Line numbers refer to that tag. One live probe was run in a herdr 0.9.3 pane hosted in iTerm2 3.6.11 (see the OSC 52 section).

## TL;DR

- **herdr has its own copy-on-select**, but **only for panes that have not enabled mouse reporting.** If a pane app turns on mouse capture (feedr does, via crossterm `EnableMouseCapture`), herdr sends every left-button down, drag and up to the app. herdr itself does no selecting there, and **no modifier bypasses this at the herdr level**.
- **Clipboard path:** herdr's own copies go through `pbcopy` on macOS. It falls back to OSC 52 on the host terminal only under SSH, WSL or VS Code remote.
- **OSC 52 from a pane app does reach the macOS clipboard** on 0.9.3. herdr intercepts it and re-emits it through that same `pbcopy` path. A live probe in this session confirmed it. **This contradicts the note in `src/tui/clipboard.rs`.** The note was probably a bad test, not a herdr limitation (see below).
- **Cmd/Super keys:** herdr *does* forward Super-modified keys to pane apps, encoded as CSI-u even for legacy-protocol panes. But on macOS the host terminal (iTerm2 here) keeps Cmd+C for its own Edit > Copy and never sends it to herdr. So feedr won't see Cmd+C unless the user remaps the key in the terminal.
- **Plugin opt-in:** there's no plugin or socket flag for "let herdr select in my pane". The only switch is the pane's own mouse mode: turn mouse reporting off and herdr's selection takes over. No socket method writes to the clipboard either. `pane.selection.read` returns text but never copies it.
- **Copy mode (`prefix+[`) works on any pane, including feedr**, whether or not the pane captures the mouse. It's keyboard-driven, and `y` copies through herdr's `pbcopy` path.

## 1. herdr's mouse selection and copy-on-select

### Lifecycle
`src/selection.rs:1-13` (module doc): MouseDown anchors, MouseDrag makes the selection active, MouseUp finalizes it, and the caller decides whether to copy.

### Trigger
In `src/client/shell/mouse.rs`, left-button **Down** on a pane (`:2198-2241`):

- `if hit.mouse_reporting && contains(inner_rect)` → `push_pane_mouse_event(...)` forwards the event to the app and starts a `pane_mouse_gesture` (`:2205-2213`). **No herdr selection is created.**
- `else` (the app hasn't enabled mouse reporting) → herdr creates `Selection::anchor(...)` (`:2214-2240`). A double click with no modifiers selects a word (`:2221-2230`).

While a `pane_mouse_gesture` is active, the following Drag and Up events go to the app (`:843-875`). With a herdr selection, Drag updates it (`:1711-1720`). On Up (`:1729-1752`):

```rust
if copied && self.config.copy_on_select {
    self.request_selection_copy(outcome, true);
    self.selection = None;
}
```

- `ui.copy_on_select` defaults to `true`, and so does `ui.mouse_capture` (`src/config/model.rs:1204-1205`).
- With `copy_on_select = false`, the selection is kept and **Ctrl+C or Cmd+C** copies it (`src/client/shell/input.rs:8-11`, `:539-554`). That key check only runs while a herdr selection is visible, so it doesn't apply to mouse-capturing panes.

`hit.mouse_reporting` comes straight from the pane's terminal mouse-protocol mode, i.e. whether the app sent DECSET 1000/1002/1003 (`src/server/client_shell.rs:534`, `src/pane/terminal.rs:151-153`).

### No modifier bypass inside herdr
The left-Down branch checks only `hit.mouse_reporting`. It never looks at modifiers before forwarding (`mouse.rs:2205`). The only configurable passthrough is for **right**-click (`ui.right_click_passthrough_modifiers`, or the `right_click: "pane"` pane flag; `mouse.rs:1755-1802`). There's nothing equivalent for left-drag selection.

The "bypass" herdr's docs mention is the **host terminal's** own: "Cmd-click is only available through the terminal-native bypass path, such as Shift-Cmd-click or `ui.mouse_capture = false`" (`docs/next/website/src/content/docs/quick-start.mdx:22`). herdr captures the mouse on the *host* terminal by default. So a native host selection (which Cmd+C would copy) needs the host terminal's mouse-reporting override. In iTerm2 that override is holding **Option (⌥)** while dragging. That's iTerm2 behaviour, not herdr's, and wasn't tested in this session. A native host selection like that is a raw screen grab, so it can cross pane borders.

### How the copy reaches the clipboard
1. `request_selection_copy` sends the socket method `pane.selection.read` with the anchor and cursor cells (`src/client/shell/actions.rs:273-298`). The server returns the text (`src/app/api/panes.rs:268-283`).
2. On the result, the client emits `ClientShellAction::ClipboardWrite(text)` and shows the "copied to clipboard" toast (`actions.rs:630-640`).
3. `shell_runtime.rs:28-29` → `crate::selection::write_osc52_bytes(&bytes)`.
4. `write_osc52_bytes` (`src/selection.rs:361-370`) calls `platform::write_clipboard` first unless `should_prefer_osc52()` is true (SSH_CONNECTION/SSH_TTY/VSCODE_IPC_HOOK_CLI/WSL; `:337-353`). Only then does it write `ESC]52;c;<b64>BEL` to the host terminal's stdout.
5. On macOS, `platform::write_clipboard` = **`pbcopy`** (`src/platform/macos.rs:631-639`). The clipboard write happens in the **client** process, which is the one attached to your terminal.

## 2. OSC 52 from a pane app (relevant to feedr's `clipboard.rs`)

herdr forwards OSC 52 writes from pane children. Commit [`6c62dba4`](https://github.com/herdrdev/herdr/commit/6c62dba4) "fix: forward OSC 52 clipboard writes from pane children" (2026-04-10) is an ancestor of v0.9.0 through v0.9.3, so every herdr version feedr has run on includes it.

- The libghostty-vt clipboard callback accepts only `text/plain`, non-empty and size-capped (`crates/ghostty-vt/src/lib.rs:640-659`). Target `c` is accepted, and BEL and ST terminators both work (test `:4820-4830`). Reads (`52;c;?`) and target `p` are ignored (test `:4516-4518`).
- The pane reader turns these into `AppEvent::ClipboardWrite` (`src/pane.rs:2470-2475`, `src/events.rs:179-181`). The headless server sends them **to the foreground client only** as `ServerMessage::Clipboard` (`src/server/headless/notifications.rs:334-339`). The client decodes them and calls the same `write_osc52_bytes` → `pbcopy` (`src/client/clipboard_forwarding.rs:9-15`).

**Live probe (this session, herdr 0.9.3 in iTerm2 3.6.11):** I wrote `ESC]52;c;ZmVlZHItb3NjNTItcHJvYmUtU1Q=ESC\` to this agent's pane pty (`/dev/ttys027`). `pbpaste` then returned `feedr-osc52-probe-ST`. **OSC 52 works through herdr on macOS.** The BEL variant wasn't probed live; the source and tests say it works.

Why feedr's earlier attempt may have failed (not verified; the attempt isn't in feedr's git history, and the first commit already uses `pbcopy`, `9e62c0e`):
- the sequence went to a stream that isn't the pane pty, or was written in a way the parser dropped;
- it used a target other than `c` (for example `p`), or an empty payload;
- the herdr client wasn't the *foreground* client (the write goes only to that client).

Either way, feedr's current `pbcopy` shell-out works whenever feedr runs on the same Mac as the herdr client. **OSC 52 would be the better path over SSH or remote herdr**, because herdr re-routes it to the machine whose client is in the foreground.

## 3. Cmd/Super keys to pane apps

- **herdr → pane:** forwarded. `src/input/encode.rs:26-36`: "Super has no legacy character encoding ... Preserve both chords with CSI-u". For a legacy-protocol pane, any Super+char chord is sent as CSI-u (`legacy_chord_needs_csi_u`, `:84-93`). For kitty-protocol panes the Super bit is 8 in the modifier field (`kitty_modifier`, `:391-404`). So feedr would get `ESC[99;9u` for Cmd+C if herdr ever received the key. crossterm decodes that as `Char('c')` + `SUPER` once feedr pushes kitty keyboard flags; for a legacy pane herdr sends CSI-u anyway. Check that crossterm parses it without the flags pushed before relying on it.
- **host terminal → herdr:** herdr pushes kitty keyboard enhancement flags to the host (`src/terminal_modes.rs:41-56`), so a terminal that *sends* Cmd chords will deliver them. herdr's own retained-selection copy key accepts `SUPER+c` "when the host terminal forwards it" (`docs/next/CHANGELOG.md:322`). The plugin docs also say that **captured mouse reports carry no Command/Super bit**, which is why link-handler clicks use Control even on macOS (`docs/next/website/src/content/docs/plugins.mdx:344-349`).
- **In practice on macOS:** iTerm2, Terminal.app and Ghostty bind Cmd+C to their own Copy menu or action, so the key never reaches herdr. That's terminal behaviour, outside herdr's source. Getting Cmd+C into feedr would take a per-user terminal remap, e.g. an iTerm2 key binding that sends `ESC[99;9u`. That can't be shipped, and it breaks normal Cmd+C everywhere else in that profile.

## 4. Can a plugin pane opt into herdr's selection, or trigger copy over the socket?

- **No explicit opt-in.** `plugin.pane.open` takes `plugin_id`, `entrypoint`, `placement`, size, `workspace_id`, `target_pane_id`, `direction`, `cwd`, `focus` and `env`. It has no mouse or selection option (`src/api/schema/plugins.rs:418-439`). The only per-pane input flag is `right_click: herdr|pane` (`pane.input.set`, `pane.split`; `src/app/api/panes.rs:108-111`, `:1472-1475`; socket-api.mdx:149-162).
- **The implicit opt-in is the pane's mouse mode.** If feedr sends `DisableMouseCapture` (DECRST 1000/1002/1003/1006), herdr selects and copy-on-selects in feedr's pane right away. But feedr then gets **no** left clicks: herdr consumes them as selection anchors (`mouse.rs:2214-2240`). Scroll still reaches the pane (`:2283-2306`).
- **Socket API:** `pane.selection.read` (`src/api/schema.rs:174-175`), `pane.read`, `pane.copy_motion` and `pane.copy_search` all *read* text. **No method writes to the clipboard.** Clipboard writes are a client-side effect; the only way in from the server side is OSC 52 from a pane child (section 2).
- **Copy mode:** `keys.copy_mode` defaults to `prefix+[` (`src/config/model.rs:1158`). It works on any focused pane's screen buffer, including mouse-capturing ones. Keys: `v` or Space to select, `y` or Enter to copy (`docs/next/website/src/content/docs/keyboard.mdx:87`). The copy goes through the same client `pbcopy` path.

## Implications for feedr: feasible copy models

| Model | Works? | Notes |
|---|---|---|
| **A. Keep mouse capture; feedr owns copy** (current modal model: in-field selection + Ctrl+C) and extend it to the list/detail view, e.g. `y` / Ctrl+C copies the focused item | Yes | Cmd+C isn't available, so bind Ctrl+C or `y`. For the clipboard write, either keep `pbcopy` (local only) or switch to **OSC 52**: verified working through herdr, and it also works when herdr is remote or over SSH. Best option: OSC 52 first, `pbcopy` as a fallback if wanted. |
| **B. Temporary "select mode"**: a key that sends `DisableMouseCapture`, lets herdr's drag select + copy-on-select run (toast included), then turns capture back on (after the next copy, on Esc, or after a timeout) | Yes | herdr does all the selection and clipboard work. feedr loses clicks while the mode is on. Rendered text gets copied, including any box-drawing characters inside the pane. |
| **C. Drop mouse capture entirely** | Yes, but costly | herdr selection works all the time, but feedr loses click-to-select and click-to-open (it keeps scroll). |
| **D. herdr copy mode (`prefix+[`)** | Yes, today, with no code | Keyboard-only. Worth documenting for users. |
| **E. Host-terminal bypass** (iTerm2 ⌥-drag, then Cmd+C) | Probably, with no code | Terminal-specific and not tested here. The raw screen selection crosses pane borders. |
| **F. Receive Cmd+C in feedr** | Not in practice | herdr forwards Super as CSI-u, but macOS terminals keep Cmd+C. It would need a per-user terminal remap. |
| **G. Ask herdr to copy over the socket** | No | No clipboard-write method exists. `pane.selection.read` only returns text. |

Recommendation: **A with OSC 52** for explicit "copy this item or field" actions. Optionally add **B** for free-form selection. Also correct the OSC 52 note in `src/tui/clipboard.rs`.
