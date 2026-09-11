use crate::config::{Side, SidebarConfig};
use crate::tui::socket::{result_entries, Herdr};
use anyhow::{Context, Result};
use serde_json::Value;

/// Terminal title the running sidebar sets so the launcher can find its pane
/// in `pane list` output (the herdr-beads OSC-marker discovery pattern).
pub const PANE_TITLE_MARKER: &str = "feedr-sidebar";

/// Shell-out seam for the herdr CLI so tests fake it.
pub trait Runner {
    /// Run `herdr <args>`, returning stdout; Err on spawn failure or non-zero exit.
    fn run(&mut self, args: &[&str]) -> Result<String>;
}

pub struct HerdrCli {
    pub bin: String,
}

impl HerdrCli {
    pub fn from_env() -> Self {
        HerdrCli {
            bin: crate::tui::socket::herdr_bin_from_env(),
        }
    }
}

impl Runner for HerdrCli {
    fn run(&mut self, args: &[&str]) -> Result<String> {
        let out = std::process::Command::new(&self.bin)
            .args(args)
            .output()
            .with_context(|| format!("cannot run {}", self.bin))?;
        if !out.status.success() {
            anyhow::bail!(
                "herdr {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

/// Idempotent dock launcher (spec §3, beads pattern): sidebar pane already
/// open → focus it (reopen ≡ focus, which also recovers after a herdr server
/// restart); otherwise split beside the focused pane → swap-walk to the edge
/// → shrink.
pub fn dock(runner: &mut dyn Runner, herdr: &mut dyn Herdr, cfg: &SidebarConfig) -> Result<String> {
    let list = runner.run(&["pane", "list"])?;
    if let Some(pane_id) = find_sidebar_pane(&list) {
        herdr.focus_pane(&pane_id)?;
        return Ok(format!("focused existing sidebar pane {pane_id}"));
    }
    let target_pane = focused_pane_id(&list)
        .context("no focused pane found in `herdr pane list` to dock the sidebar beside")?;
    let pane_id = open_split_dock(runner, cfg, &target_pane, false)?;
    Ok(format!("opened sidebar pane {pane_id}"))
}

/// Plan 3: idempotent auto-dock for herdr-plugin.toml's `tab.created`
/// `[[events]]` hook. Scoped to the ONE tab named by `tab_id` (the just-
/// created tab, per herdr's `HERDR_TAB_ID` event-hook context — see
/// scripts/on-tab-created.sh) — a sidebar pane open in some OTHER tab does
/// not satisfy this, unlike the workspace-wide `dock()` used by the
/// open-feedr action. A pane already docked in this tab is left alone
/// (no-op — this fires unattended, so it deliberately does not steal focus
/// the way the user-initiated `dock()`/`herdr.focus_pane` path does).
pub fn auto_dock_for_tab(
    runner: &mut dyn Runner,
    cfg: &SidebarConfig,
    tab_id: &str,
) -> Result<String> {
    let list = runner.run(&["pane", "list"])?;
    let v: Value = serde_json::from_str(&list).unwrap_or(Value::Null);
    let result = v.get("result").unwrap_or(&v);
    let entries = result_entries(result);

    if let Some(pane_id) = entries
        .iter()
        .find_map(|e| sidebar_pane_id_in_tab(e, tab_id))
    {
        return Ok(format!(
            "tab {tab_id} already has sidebar pane {pane_id}; no-op"
        ));
    }

    // A pane already in the new tab to split beside (herdr's tab.create
    // always spawns an initial shell pane — verified via socket.rs's
    // create_tab/UnixSocketClient tests, which resolve exactly this way).
    let target_pane = entries
        .iter()
        .find_map(|e| {
            (e.get("tab_id").and_then(|t| t.as_str()) == Some(tab_id))
                .then(|| e.get("pane_id")?.as_str().map(str::to_string))
                .flatten()
        })
        .with_context(|| format!("no pane found in tab {tab_id} to dock the sidebar beside"))?;

    // `--no-focus`: this hook fires unattended on every new tab, so it must
    // not yank focus away from wherever the user (or another agent) already
    // is — unlike the user-initiated `dock()` path above.
    let pane_id = open_split_dock(runner, cfg, &target_pane, true)?;
    Ok(format!(
        "auto-docked sidebar pane {pane_id} in tab {tab_id}"
    ))
}

/// herdr-feedr's own plugin id / pane entrypoint (herdr-plugin.toml) — the
/// preferred dock path runs the sidebar this way rather than by execing a
/// bare `feedr` (Task: `feedr` is not on PATH, so the old `pane split` +
/// `send-text "exec feedr sidebar"` path spawned a shell that died
/// instantly and the pane closed under it).
const PLUGIN_ID: &str = "herdr-feedr";
const PLUGIN_ENTRYPOINT: &str = "feedr-sidebar";

/// Shared tail of both dock paths above: open the sidebar pane beside
/// `target_pane` — PREFERRED via `herdr plugin pane open`, which launches
/// the entrypoint from the installed plugin's own root regardless of PATH;
/// FALLBACK (when `plugin pane open` errors — e.g. this binary invoked
/// outside an installed plugin, or the plugin isn't registered) via `pane
/// split` + `send-text` execing *this running binary's absolute path*
/// (`std::env::current_exe()`) instead of a bare `feedr` — then swap-walk it
/// to `cfg.side`'s edge and shrink it to `cfg.width`. Returns the new pane's
/// id.
fn open_split_dock(
    runner: &mut dyn Runner,
    cfg: &SidebarConfig,
    target_pane: &str,
    no_focus: bool,
) -> Result<String> {
    let pane_id = match open_via_plugin_pane(runner, target_pane, no_focus) {
        Ok(pane_id) => pane_id,
        Err(_plugin_pane_open_err) => open_via_split_fallback(runner, target_pane, no_focus)?,
    };
    if cfg.side == Side::Left {
        for _ in 0..6 {
            if runner
                .run(&[
                    "pane",
                    "neighbor",
                    "--direction",
                    "left",
                    "--pane",
                    &pane_id,
                ])
                .is_err()
            {
                break; // at the left edge
            }
            if runner
                .run(&["pane", "swap", "--direction", "left", "--pane", &pane_id])
                .is_err()
            {
                break;
            }
        }
    }
    let dir = match cfg.side {
        Side::Left => "left",
        Side::Right => "right",
    };
    let amount = format!("{}", cfg.width);
    runner.run(&[
        "pane",
        "resize",
        "--direction",
        dir,
        "--amount",
        &amount,
        "--pane",
        &pane_id,
    ])?;
    Ok(pane_id)
}

/// PREFERRED path: `herdr plugin pane open` — verified live against herdr
/// 0.9.0 with herdr-feedr installed as a local plugin (`herdr plugin pane
/// open --plugin herdr-feedr --entrypoint feedr-sidebar --placement split
/// --direction right --target-pane <PANE_ID>`). Its JSON result nests the
/// new pane under `result.plugin_pane.pane.pane_id` (not flat like `pane
/// split`'s), so this reuses the same structural `find_string_field` search
/// as the fallback below rather than assuming a shape.
fn open_via_plugin_pane(
    runner: &mut dyn Runner,
    target_pane: &str,
    no_focus: bool,
) -> Result<String> {
    let mut args = vec![
        "plugin",
        "pane",
        "open",
        "--plugin",
        PLUGIN_ID,
        "--entrypoint",
        PLUGIN_ENTRYPOINT,
        "--placement",
        "split",
        "--direction",
        "right",
        "--target-pane",
        target_pane,
    ];
    if no_focus {
        args.push("--no-focus");
    }
    let out = runner.run(&args)?;
    crate::tui::socket::find_string_field(
        &serde_json::from_str::<Value>(&out).unwrap_or(Value::Null),
        "pane_id",
    )
    .context("no pane_id in `herdr plugin pane open` output")
}

/// FALLBACK path, used when `open_via_plugin_pane` errors (e.g. this binary
/// run outside an installed plugin, or the plugin isn't registered):
/// `pane.split` spawns a plain shell (no command param in 0.9.0) and splits
/// only right/down, so split right then exec the sidebar into the new shell
/// by its *absolute path* — `feedr` alone is not guaranteed to be on PATH in
/// that shell, which is exactly what made this path silently die before
/// (the shell exited instantly and the pane closed under it).
fn open_via_split_fallback(
    runner: &mut dyn Runner,
    target_pane: &str,
    no_focus: bool,
) -> Result<String> {
    let mut split_args = vec![
        "pane",
        "split",
        "--pane",
        target_pane,
        "--direction",
        "right",
    ];
    if no_focus {
        split_args.push("--no-focus");
    }
    let out = runner.run(&split_args)?;
    let pane_id = crate::tui::socket::find_string_field(
        &serde_json::from_str::<Value>(&out).unwrap_or(Value::Null),
        "pane_id",
    )
    .context("no pane_id in `herdr pane split` output")?;
    let exe = std::env::current_exe().context("cannot resolve this binary's own path")?;
    // Single-quoted so a path containing spaces still execs correctly;
    // `exe` is our own resolved path, not untrusted input.
    let exec_cmd = format!("exec '{}' sidebar\n", exe.display());
    // `herdr pane send-text --help` (0.9.0): positional `<PANE_ID> <TEXT>`,
    // not the --pane/--text flags the plan assumed — adapted here.
    if let Err(e) = runner.run(&["pane", "send-text", &pane_id, &exec_cmd]) {
        // Best-effort cleanup: the split succeeded but the pane never got the
        // sidebar exec'd into it, so it's an orphan empty shell pane. Ignore
        // the close result (nothing more useful to do if it fails too) and
        // propagate the original error.
        let _ = runner.run(&["pane", "close", &pane_id]);
        return Err(e);
    }
    Ok(pane_id)
}

/// The currently-focused pane's id, if any — the target the user-initiated
/// `dock()` path splits beside (unlike `auto_dock_for_tab`, which already
/// knows its target: the lone pane in the just-created tab).
fn focused_pane_id(pane_list_stdout: &str) -> Option<String> {
    let v: Value = serde_json::from_str(pane_list_stdout).ok()?;
    let result = v.get("result").unwrap_or(&v);
    result_entries(result).into_iter().find_map(|e| {
        if e.get("focused")?.as_bool()? {
            e.get("pane_id")?.as_str().map(str::to_string)
        } else {
            None
        }
    })
}

fn find_sidebar_pane(pane_list_stdout: &str) -> Option<String> {
    let v: Value = serde_json::from_str(pane_list_stdout).ok()?;
    let result = v.get("result").unwrap_or(&v);
    result_entries(result).into_iter().find_map(sidebar_pane_id)
}

/// A pane entry's id, if its title is exactly the running sidebar's marker.
/// Exact match, not `contains` — the running sidebar sets its title to
/// exactly PANE_TITLE_MARKER (mod.rs's SetTitle call), so an exact check
/// can't be fooled by e.g. an editor session on a file named
/// "feedr-sidebar-notes.md" (Task 13 review rider).
fn sidebar_pane_id(e: &Value) -> Option<String> {
    let title = e
        .get("terminal_title_stripped")
        .or_else(|| e.get("terminal_title"))?
        .as_str()?;
    if title == PANE_TITLE_MARKER {
        Some(e.get("pane_id")?.as_str()?.to_string())
    } else {
        None
    }
}

/// Same match as `sidebar_pane_id`, additionally scoped to one tab (Plan 3's
/// per-tab auto-dock idempotency check).
fn sidebar_pane_id_in_tab(e: &Value, tab_id: &str) -> Option<String> {
    if e.get("tab_id").and_then(|t| t.as_str()) != Some(tab_id) {
        return None;
    }
    sidebar_pane_id(e)
}

/// Bound on repeated-resize loops (both directions of the «/» toggle):
/// verified against a live `herdr pane resize --help` (0.9.0) — resize
/// amounts are relative proportional shares, not columns, and there is no
/// "resize to minimum" verb, so driving a pane to herdr's own minimum width
/// means repeating the same delta until herdr itself reports no more room.
/// Six matches the existing swap-walk bound in `dock()` (a workspace won't
/// realistically have more splits than that to cross).
const MAX_COLLAPSE_STEPS: u32 = 6;

/// A left-edge pane's movable edge is its right one: shrinking it means
/// resizing left; a right-edge pane shrinks by resizing right.
fn collapse_direction(side: Side) -> &'static str {
    match side {
        Side::Left => "left",
        Side::Right => "right",
    }
}

/// `herdr pane resize`'s JSON result carries `"changed": bool` (verified via
/// `herdr api schema --json`'s `PaneResizeResult`) — `false` (or a
/// `"reason":"unchanged"`) means the pane was already at herdr's own
/// minimum/edge and this call was a no-op. Missing/unparsable output is
/// treated the same as `false`: stop rather than loop on a response we can't
/// actually confirm changed anything.
fn resize_changed(out: &str) -> bool {
    serde_json::from_str::<Value>(out)
        .ok()
        .and_then(|v| v.get("result").unwrap_or(&v).get("changed")?.as_bool())
        .unwrap_or(false)
}

/// Shrink the pane toward `cfg.side`'s edge, repeating the same relative
/// delta (bounded by `MAX_COLLAPSE_STEPS`) until herdr reports no more
/// change (at its minimum width) or a resize call errors. Returns the number
/// of resizes that actually changed the pane's width — the caller must save
/// this so `expand_pane` can undo exactly that many steps later, rather than
/// approximating with a single fixed-count inverse (Plan 3b: the old
/// one-shot `resize_for_collapse` left the pane visibly still wide).
pub fn collapse_pane(runner: &mut dyn Runner, cfg: &SidebarConfig, pane_id: &str) -> usize {
    let dir = collapse_direction(cfg.side);
    let amount = format!("{}", cfg.width);
    let mut applied = 0usize;
    for _ in 0..MAX_COLLAPSE_STEPS {
        match runner.run(&[
            "pane",
            "resize",
            "--direction",
            dir,
            "--amount",
            &amount,
            "--pane",
            pane_id,
        ]) {
            Ok(out) if resize_changed(&out) => applied += 1,
            _ => break,
        }
    }
    applied
}

/// Inverse of `collapse_pane`: resize the opposite direction exactly `steps`
/// times (the count `collapse_pane` actually applied — not the bound, and
/// not a single guess), restoring the pane to its pre-collapse width.
/// Best-effort: an error mid-sequence just stops early, matching the
/// existing "the rendered rail still toggles without herdr" degrade policy.
pub fn expand_pane(runner: &mut dyn Runner, cfg: &SidebarConfig, pane_id: &str, steps: usize) {
    let dir = collapse_direction(match cfg.side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    });
    let amount = format!("{}", cfg.width);
    for _ in 0..steps {
        if runner
            .run(&[
                "pane",
                "resize",
                "--direction",
                dir,
                "--amount",
                &amount,
                "--pane",
                pane_id,
            ])
            .is_err()
        {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Side, SidebarConfig};
    use crate::tui::socket::FakeHerdr;

    fn cfg(side: Side) -> SidebarConfig {
        SidebarConfig {
            side,
            width: 0.18,
            auto_dock: false,
        }
    }

    struct FakeRunner {
        calls: Vec<String>,
        responses: std::collections::VecDeque<anyhow::Result<String>>,
    }

    impl FakeRunner {
        fn new(responses: Vec<anyhow::Result<String>>) -> Self {
            FakeRunner {
                calls: Vec::new(),
                responses: responses.into(),
            }
        }
    }

    impl Runner for FakeRunner {
        fn run(&mut self, args: &[&str]) -> anyhow::Result<String> {
            self.calls.push(args.join(" "));
            self.responses.pop_front().unwrap_or(Ok(String::new()))
        }
    }

    #[test]
    fn dock_focuses_existing_sidebar_pane() {
        let list = r#"{"id":"cli:pane:list","result":{"panes":[
            {"pane_id":"w1:p2","terminal_title_stripped":"vim"},
            {"pane_id":"w1:p3","terminal_title_stripped":"feedr-sidebar"}]}}"#;
        let mut runner = FakeRunner::new(vec![Ok(list.into())]);
        let mut herdr = FakeHerdr::default();
        let msg = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap();
        assert!(msg.contains("focused"));
        assert_eq!(herdr.log.borrow().as_slice(), ["focus_pane w1:p3"]);
        assert_eq!(runner.calls, ["pane list"]); // idempotent: nothing opened
    }

    /// A pane found via `focused: true` in `pane list`, to split beside.
    fn focused_list() -> &'static str {
        r#"{"id":"cli:pane:list","result":{"panes":[
            {"pane_id":"w1:p1","terminal_title_stripped":"vim","focused":true}]}}"#
    }

    #[test]
    fn dock_opens_via_plugin_pane_swap_walks_left_and_resizes() {
        let opened = r#"{"id":"cli:plugin","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(focused_list().into()),
            Ok(opened.into()),
            Ok(String::new()),                   // neighbor 1: one exists
            Ok(String::new()),                   // swap 1
            Err(anyhow::anyhow!("no neighbor")), // neighbor 2: at the edge
        ]);
        let mut herdr = FakeHerdr::default();
        let msg = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap();
        assert!(msg.contains("opened sidebar pane w1:p9"));
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
                 --placement split --direction right --target-pane w1:p1",
                "pane neighbor --direction left --pane w1:p9",
                "pane swap --direction left --pane w1:p9",
                "pane neighbor --direction left --pane w1:p9",
                "pane resize --direction left --amount 0.18 --pane w1:p9",
            ]
        );
        assert!(herdr.log.borrow().is_empty());
    }

    #[test]
    fn dock_right_side_skips_swap_walk() {
        let opened = r#"{"id":"cli:plugin","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![Ok(focused_list().into()), Ok(opened.into())]);
        let mut herdr = FakeHerdr::default();
        dock(&mut runner, &mut herdr, &cfg(Side::Right)).unwrap();
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
                 --placement split --direction right --target-pane w1:p1",
                "pane resize --direction right --amount 0.18 --pane w1:p9",
            ]
        );
    }

    #[test]
    fn dock_errors_when_no_pane_is_focused() {
        let list = r#"{"id":"x","result":{"panes":[
            {"pane_id":"w1:p1","terminal_title_stripped":"vim"}]}}"#;
        let mut runner = FakeRunner::new(vec![Ok(list.into())]);
        let mut herdr = FakeHerdr::default();
        let err = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap_err();
        assert!(err.to_string().contains("focused"), "got: {err}");
        assert_eq!(runner.calls, ["pane list"]); // nothing opened
    }

    /// Real herdr 0.9.0 nests the new pane's id under
    /// `result.plugin_pane.pane.pane_id` (captured live against an installed
    /// herdr-feedr plugin), unlike `pane split`'s flat `result.pane_id` — the
    /// structural `find_string_field` search handles both without caring.
    #[test]
    fn open_via_plugin_pane_parses_the_real_nested_pane_id_shape() {
        let out = r#"{"id":"cli:plugin","result":{"plugin_pane":{"entrypoint":"feedr-sidebar",
            "pane":{"pane_id":"w3:pF","tab_id":"w3:t6"},"plugin_id":"herdr-feedr"},
            "type":"plugin_pane_opened"}}"#;
        let mut runner = FakeRunner::new(vec![Ok(out.into())]);
        let pane_id = open_via_plugin_pane(&mut runner, "w3:pB", false).unwrap();
        assert_eq!(pane_id, "w3:pF");
        assert_eq!(
            runner.calls,
            [
                "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
              --placement split --direction right --target-pane w3:pB"
            ]
        );
    }

    #[test]
    fn dock_falls_back_to_absolute_path_exec_when_plugin_pane_open_fails() {
        let split = r#"{"id":"y","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(focused_list().into()),
            Err(anyhow::anyhow!("plugin herdr-feedr not found")),
            Ok(split.into()),
            Ok(String::new()),                   // send-text
            Err(anyhow::anyhow!("no neighbor")), // edge (single split)
        ]);
        let mut herdr = FakeHerdr::default();
        let msg = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap();
        assert!(msg.contains("opened sidebar pane w1:p9"), "got: {msg}");
        let exe = std::env::current_exe().unwrap();
        let expected_exec = format!("exec '{}' sidebar\n", exe.display());
        assert_eq!(
            runner.calls,
            [
                "pane list".to_string(),
                "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
                 --placement split --direction right --target-pane w1:p1"
                    .to_string(),
                "pane split --pane w1:p1 --direction right".to_string(),
                format!("pane send-text w1:p9 {expected_exec}"),
                "pane neighbor --direction left --pane w1:p9".to_string(),
                "pane resize --direction left --amount 0.18 --pane w1:p9".to_string(),
            ]
        );
    }

    #[test]
    fn find_sidebar_pane_requires_exact_marker_match() {
        // A pane titled by e.g. an editor session on a file whose name
        // happens to contain the marker string must NOT be mistaken for the
        // running sidebar (rider on Task 13 review). Put the false-positive
        // candidate first so a `.contains()`-based match would pick it.
        let list = r#"{"id":"x","result":{"panes":[
            {"pane_id":"w1:p1","terminal_title_stripped":"vim feedr-sidebar-notes.md"},
            {"pane_id":"w1:p2","terminal_title_stripped":"feedr-sidebar"}]}}"#;
        assert_eq!(find_sidebar_pane(list), Some("w1:p2".to_string()));
    }

    // --- Plan 3: auto_dock_for_tab (tab.created event hook) ----------------

    #[test]
    fn auto_dock_for_tab_is_a_noop_when_this_tab_already_has_a_sidebar_pane() {
        let list = r#"{"id":"x","result":{"panes":[
            {"pane_id":"w1:p1","tab_id":"w1:t2","terminal_title_stripped":"feedr-sidebar"},
            {"pane_id":"w1:p2","tab_id":"w1:t9","terminal_title_stripped":"vim"}]}}"#;
        let mut runner = FakeRunner::new(vec![Ok(list.into())]);
        let msg = auto_dock_for_tab(&mut runner, &cfg(Side::Left), "w1:t2").unwrap();
        assert!(msg.contains("no-op"), "got: {msg}");
        assert_eq!(runner.calls, ["pane list"]); // nothing opened, nothing focused
    }

    /// A sidebar pane open in a DIFFERENT tab must not satisfy this tab's
    /// idempotency check — auto-dock is scoped per-tab, unlike `dock()`.
    #[test]
    fn auto_dock_for_tab_docks_even_when_another_tab_already_has_a_sidebar_pane() {
        let list = r#"{"id":"x","result":{"panes":[
            {"pane_id":"w1:p1","tab_id":"w1:t2","terminal_title_stripped":"feedr-sidebar"},
            {"pane_id":"w1:p9","tab_id":"w1:t9","terminal_title_stripped":"vim"}]}}"#;
        let opened = r#"{"id":"y","result":{"pane_id":"w1:p10"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(list.into()),
            Ok(opened.into()),
            Err(anyhow::anyhow!("no neighbor")), // neighbor: at the edge (single-pane new tab)
        ]);
        let msg = auto_dock_for_tab(&mut runner, &cfg(Side::Left), "w1:t9").unwrap();
        assert!(msg.contains("auto-docked"), "got: {msg}");
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
                 --placement split --direction right --target-pane w1:p9 --no-focus",
                "pane neighbor --direction left --pane w1:p10",
                "pane resize --direction left --amount 0.18 --pane w1:p10",
            ]
        );
    }

    #[test]
    fn auto_dock_for_tab_falls_back_to_absolute_path_exec_when_plugin_pane_open_fails() {
        let list = r#"{"id":"x","result":{"panes":[
            {"pane_id":"w1:p9","tab_id":"w1:t9","terminal_title_stripped":"vim"}]}}"#;
        let split = r#"{"id":"y","result":{"pane_id":"w1:p10"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(list.into()),
            Err(anyhow::anyhow!("plugin herdr-feedr not found")),
            Ok(split.into()),
            Ok(String::new()),                   // send-text
            Err(anyhow::anyhow!("no neighbor")), // edge (single-pane new tab)
        ]);
        let msg = auto_dock_for_tab(&mut runner, &cfg(Side::Left), "w1:t9").unwrap();
        assert!(msg.contains("auto-docked"), "got: {msg}");
        let exe = std::env::current_exe().unwrap();
        let expected_exec = format!("exec '{}' sidebar\n", exe.display());
        assert_eq!(
            runner.calls,
            [
                "pane list".to_string(),
                "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
                 --placement split --direction right --target-pane w1:p9 --no-focus"
                    .to_string(),
                "pane split --pane w1:p9 --direction right --no-focus".to_string(),
                format!("pane send-text w1:p10 {expected_exec}"),
                "pane neighbor --direction left --pane w1:p10".to_string(),
                "pane resize --direction left --amount 0.18 --pane w1:p10".to_string(),
            ]
        );
    }

    #[test]
    fn auto_dock_for_tab_errors_when_the_tab_has_no_panes() {
        let list = r#"{"id":"x","result":{"panes":[
            {"pane_id":"w1:p1","tab_id":"w1:t2","terminal_title_stripped":"vim"}]}}"#;
        let mut runner = FakeRunner::new(vec![Ok(list.into())]);
        let err = auto_dock_for_tab(&mut runner, &cfg(Side::Left), "w1:t9").unwrap_err();
        assert!(err.to_string().contains("w1:t9"), "got: {err}");
    }

    #[test]
    fn dock_closes_orphan_pane_when_fallback_send_text_fails() {
        // Plugin pane open fails (falls back to split), the split succeeds,
        // but the exec into it fails too: the orphan empty shell pane must
        // still be cleaned up.
        let split = r#"{"id":"cli:pane:split","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(focused_list().into()),
            Err(anyhow::anyhow!("plugin herdr-feedr not found")),
            Ok(split.into()),
            Err(anyhow::anyhow!("send-text failed")),
            Ok(String::new()), // pane close — result ignored
        ]);
        let mut herdr = FakeHerdr::default();
        let result = dock(&mut runner, &mut herdr, &cfg(Side::Left));
        assert!(result.is_err());
        assert_eq!(runner.calls.len(), 5);
        assert_eq!(runner.calls[0], "pane list");
        assert_eq!(
            runner.calls[1],
            "plugin pane open --plugin herdr-feedr --entrypoint feedr-sidebar \
             --placement split --direction right --target-pane w1:p1"
        );
        assert_eq!(runner.calls[2], "pane split --pane w1:p1 --direction right");
        assert!(
            runner.calls[3].starts_with("pane send-text w1:p9 exec '")
                && runner.calls[3].ends_with("sidebar\n"),
            "got: {}",
            runner.calls[3]
        );
        assert_eq!(runner.calls[4], "pane close w1:p9");
    }

    #[test]
    fn dock_errors_when_both_plugin_pane_open_and_fallback_split_fail() {
        let mut runner = FakeRunner::new(vec![
            Ok(focused_list().into()),
            Err(anyhow::anyhow!("plugin herdr-feedr not found")),
            Err(anyhow::anyhow!("split failed")),
        ]);
        let mut herdr = FakeHerdr::default();
        let err = dock(&mut runner, &mut herdr, &cfg(Side::Left)).unwrap_err();
        assert!(err.to_string().contains("split failed"), "got: {err}");
    }

    fn changed(v: bool) -> anyhow::Result<String> {
        Ok(format!(r#"{{"result":{{"changed":{v}}}}}"#))
    }

    #[test]
    fn collapse_pane_resizes_until_herdr_reports_no_change() {
        // Two real shrinks, then herdr reports the pane is already at its
        // minimum — the loop must stop there, not keep spinning to the bound.
        let mut r = FakeRunner::new(vec![changed(true), changed(true), changed(false)]);
        let applied = collapse_pane(&mut r, &cfg(Side::Left), "w1:p3");
        assert_eq!(applied, 2, "only the two changed resizes count");
        assert_eq!(
            r.calls,
            vec!["pane resize --direction left --amount 0.18 --pane w1:p3"; 3]
        );
    }

    #[test]
    fn collapse_pane_stops_at_the_bound_when_herdr_keeps_reporting_changed() {
        // Pathological herdr that always reports "changed": true — the loop
        // must still stop after MAX_COLLAPSE_STEPS, never spin forever.
        let mut r = FakeRunner::new((0..10).map(|_| changed(true)).collect());
        let applied = collapse_pane(&mut r, &cfg(Side::Left), "w1:p3");
        assert_eq!(applied, MAX_COLLAPSE_STEPS as usize);
        assert_eq!(r.calls.len(), MAX_COLLAPSE_STEPS as usize);
    }

    #[test]
    fn collapse_pane_stops_on_error() {
        let mut r = FakeRunner::new(vec![
            changed(true),
            changed(true),
            Err(anyhow::anyhow!("herdr socket unavailable")),
        ]);
        let applied = collapse_pane(&mut r, &cfg(Side::Left), "w1:p3");
        assert_eq!(applied, 2);
        assert_eq!(r.calls.len(), 3);
    }

    #[test]
    fn collapse_pane_direction_by_side() {
        let mut r = FakeRunner::new(vec![changed(false)]);
        collapse_pane(&mut r, &cfg(Side::Left), "w1:p3");
        assert_eq!(
            r.calls,
            ["pane resize --direction left --amount 0.18 --pane w1:p3"]
        );

        let mut r = FakeRunner::new(vec![changed(false)]);
        collapse_pane(&mut r, &cfg(Side::Right), "w1:p3");
        assert_eq!(
            r.calls,
            ["pane resize --direction right --amount 0.18 --pane w1:p3"]
        );
    }

    #[test]
    fn expand_pane_applies_exactly_the_given_count_in_the_inverse_direction() {
        let mut r = FakeRunner::new((0..3).map(|_| Ok(String::new())).collect());
        expand_pane(&mut r, &cfg(Side::Left), "w1:p3", 3);
        assert_eq!(
            r.calls,
            vec!["pane resize --direction right --amount 0.18 --pane w1:p3"; 3]
        );

        let mut r = FakeRunner::new((0..2).map(|_| Ok(String::new())).collect());
        expand_pane(&mut r, &cfg(Side::Right), "w1:p3", 2);
        assert_eq!(
            r.calls,
            vec!["pane resize --direction left --amount 0.18 --pane w1:p3"; 2]
        );
    }

    #[test]
    fn expand_pane_zero_steps_calls_herdr_nothing() {
        let mut r = FakeRunner::new(vec![]);
        expand_pane(&mut r, &cfg(Side::Left), "w1:p3", 0);
        assert!(r.calls.is_empty());
    }

    #[test]
    fn expand_pane_stops_early_on_error() {
        let mut r = FakeRunner::new(vec![Ok(String::new()), Err(anyhow::anyhow!("boom"))]);
        // Asked for 5 but only 2 responses queued; FakeRunner returns Ok("")
        // once responses run out, so scope the assertion to what we scripted.
        expand_pane(&mut r, &cfg(Side::Left), "w1:p3", 2);
        assert_eq!(r.calls.len(), 2, "stops as soon as a resize call errors");
    }
}
