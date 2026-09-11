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

/// Best-effort pane resize behind the «/» toggle. Plugin v1 resize amounts
/// are relative, so "restore previous width" is the inverse delta —
/// approximate by design (research doc). Callers ignore errors (the rendered
/// rail still collapses without herdr).
pub fn resize_for_collapse(
    runner: &mut dyn Runner,
    cfg: &SidebarConfig,
    collapsed: bool,
    pane_id: &str,
) -> Result<()> {
    // A left-edge pane's movable edge is its right one: shrink = resize left.
    let dir = match (cfg.side, collapsed) {
        (Side::Left, true) | (Side::Right, false) => "left",
        _ => "right",
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
        pane_id,
    ])?;
    Ok(())
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

    #[test]
    fn collapse_resize_uses_inverse_deltas() {
        let mut r = FakeRunner::new(vec![]);
        resize_for_collapse(&mut r, &cfg(Side::Left), true, "w1:p3").unwrap();
        resize_for_collapse(&mut r, &cfg(Side::Left), false, "w1:p3").unwrap();
        resize_for_collapse(&mut r, &cfg(Side::Right), true, "w1:p3").unwrap();
        assert_eq!(
            r.calls,
            [
                "pane resize --direction left --amount 0.18 --pane w1:p3",
                "pane resize --direction right --amount 0.18 --pane w1:p3",
                "pane resize --direction right --amount 0.18 --pane w1:p3",
            ]
        );
    }
}
