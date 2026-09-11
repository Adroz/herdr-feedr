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
/// restart); otherwise split → swap-walk to the edge → shrink.
pub fn dock(runner: &mut dyn Runner, herdr: &mut dyn Herdr, cfg: &SidebarConfig) -> Result<String> {
    let list = runner.run(&["pane", "list"])?;
    if let Some(pane_id) = find_sidebar_pane(&list) {
        herdr.focus_pane(&pane_id)?;
        return Ok(format!("focused existing sidebar pane {pane_id}"));
    }
    // pane.split spawns a plain shell (no command param in 0.9.0) and splits
    // only right/down: split right, exec the sidebar into the new shell,
    // then swap-walk left.
    let out = runner.run(&["pane", "split", "--direction", "right"])?;
    let pane_id = crate::tui::socket::find_string_field(
        &serde_json::from_str::<Value>(&out).unwrap_or(Value::Null),
        "pane_id",
    )
    .context("no pane_id in `herdr pane split` output")?;
    // `herdr pane send-text --help` (0.9.0): positional `<PANE_ID> <TEXT>`,
    // not the --pane/--text flags the plan assumed — adapted here.
    if let Err(e) = runner.run(&["pane", "send-text", &pane_id, "exec feedr sidebar\n"]) {
        // Best-effort cleanup: the split succeeded but the pane never got the
        // sidebar exec'd into it, so it's an orphan empty shell pane. Ignore
        // the close result (nothing more useful to do if it fails too) and
        // propagate the original error.
        let _ = runner.run(&["pane", "close", &pane_id]);
        return Err(e);
    }
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
    Ok(format!("opened sidebar pane {pane_id}"))
}

fn find_sidebar_pane(pane_list_stdout: &str) -> Option<String> {
    let v: Value = serde_json::from_str(pane_list_stdout).ok()?;
    let result = v.get("result").unwrap_or(&v);
    result_entries(result).into_iter().find_map(|e| {
        let title = e
            .get("terminal_title_stripped")
            .or_else(|| e.get("terminal_title"))?
            .as_str()?;
        // Exact match, not `contains` — the running sidebar sets its title
        // to exactly PANE_TITLE_MARKER (mod.rs's SetTitle call), so an exact
        // check can't be fooled by e.g. an editor session on a file named
        // "feedr-sidebar-notes.md" (Task 13 review rider).
        if title == PANE_TITLE_MARKER {
            Some(e.get("pane_id")?.as_str()?.to_string())
        } else {
            None
        }
    })
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

    #[test]
    fn dock_opens_execs_sidebar_swap_walks_left_and_resizes() {
        let empty = r#"{"id":"cli:pane:list","result":{"panes":[]}}"#;
        let split = r#"{"id":"cli:pane:split","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(empty.into()),
            Ok(split.into()),
            Ok(String::new()),                   // send-text
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
                "pane split --direction right",
                "pane send-text w1:p9 exec feedr sidebar\n",
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
        let empty = r#"{"id":"cli:pane:list","result":{"panes":[]}}"#;
        let split = r#"{"id":"cli:pane:split","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![Ok(empty.into()), Ok(split.into())]);
        let mut herdr = FakeHerdr::default();
        dock(&mut runner, &mut herdr, &cfg(Side::Right)).unwrap();
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "pane split --direction right",
                "pane send-text w1:p9 exec feedr sidebar\n",
                "pane resize --direction right --amount 0.18 --pane w1:p9",
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

    #[test]
    fn dock_closes_orphan_pane_when_send_text_fails() {
        let empty = r#"{"id":"cli:pane:list","result":{"panes":[]}}"#;
        let split = r#"{"id":"cli:pane:split","result":{"pane_id":"w1:p9"}}"#;
        let mut runner = FakeRunner::new(vec![
            Ok(empty.into()),
            Ok(split.into()),
            Err(anyhow::anyhow!("send-text failed")),
            Ok(String::new()), // pane close — result ignored
        ]);
        let mut herdr = FakeHerdr::default();
        let result = dock(&mut runner, &mut herdr, &cfg(Side::Left));
        assert!(result.is_err());
        assert_eq!(
            runner.calls,
            [
                "pane list",
                "pane split --direction right",
                "pane send-text w1:p9 exec feedr sidebar\n",
                "pane close w1:p9",
            ]
        );
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
