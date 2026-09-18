//! Who is claiming? (SPEC §4, [#11](https://github.com/Adroz/herdr-feedr/issues/11))
//!
//! The agent ref that `claim` stamps resolves from four sources, in order:
//! `--agent` > `FEEDR_AGENT` > `herdr pane current` (only inside herdr) >
//! `CLAUDE_CODE_SESSION_ID`. The last two are verified to agree — herdr's
//! `agent_session.value` *is* the Claude session id — so the common case
//! needs no socket call and identity still resolves outside herdr.
//!
//! When nothing resolves, this fails loudly: an unlinked claim is a dead link
//! the sidebar can't jump from, so `claim` must never write one.

use crate::feed::AgentRef;
use crate::tui::dock::Runner;
use anyhow::{anyhow, Result};

/// Where a resolved [`Identity`] came from — reported by `feedr whoami` so a
/// surprising ref can be traced to the thing that supplied it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Flag,
    Env,
    HerdrPane,
    ClaudeSession,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Flag => "--agent",
            Source::Env => "FEEDR_AGENT",
            Source::HerdrPane => "herdr pane current",
            Source::ClaudeSession => "CLAUDE_CODE_SESSION_ID",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub agent: AgentRef,
    pub source: Source,
}

/// Environment lookup, injected so the resolution order is testable without
/// touching the process environment.
pub trait Env {
    fn get(&self, key: &str) -> Option<String>;
}

/// The real environment.
pub struct SystemEnv;

impl Env for SystemEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok().filter(|v| !v.is_empty())
    }
}

pub fn resolve(flag: Option<&str>, env: &dyn Env, runner: &mut dyn Runner) -> Result<Identity> {
    // A ref that was *supplied* but doesn't parse is a mistake to report, not a
    // source to skip: falling through would silently claim as someone else.
    if let Some(raw) = flag {
        let agent = AgentRef::parse(raw)
            .ok_or_else(|| anyhow!("--agent must be kind:id, got \"{raw}\""))?;
        return Ok(Identity {
            agent,
            source: Source::Flag,
        });
    }
    if let Some(raw) = env.get("FEEDR_AGENT") {
        let agent = AgentRef::parse(&raw)
            .ok_or_else(|| anyhow!("FEEDR_AGENT must be kind:id, got \"{raw}\""))?;
        return Ok(Identity {
            agent,
            source: Source::Env,
        });
    }
    // Only inside herdr: outside it there's no socket to answer, and shelling
    // out just to fail costs a process per claim.
    if env.get("HERDR_ENV").is_some() {
        if let Some(agent) = runner.run(&["pane", "current"]).ok().and_then(|out| {
            let v: serde_json::Value = serde_json::from_str(&out).ok()?;
            let pane = v.get("result")?.get("pane")?;
            let kind = pane.get("agent")?.as_str()?;
            let id = pane.get("agent_session")?.get("value")?.as_str()?;
            AgentRef::parse(&format!("{kind}:{id}"))
        }) {
            return Ok(Identity {
                agent,
                source: Source::HerdrPane,
            });
        }
    }
    if let Some(id) = env.get("CLAUDE_CODE_SESSION_ID") {
        // herdr's `agent_session.value` is this same id, so the two agree by
        // construction and a claim made outside herdr still joins inside it.
        return Ok(Identity {
            agent: AgentRef {
                kind: "claude".to_string(),
                id,
            },
            source: Source::ClaudeSession,
        });
    }
    Err(anyhow!(
        "cannot tell which agent is claiming — tried --agent, FEEDR_AGENT, \
         herdr pane current, CLAUDE_CODE_SESSION_ID. Pass --agent kind:id."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    struct FakeEnv(HashMap<&'static str, &'static str>);

    impl FakeEnv {
        fn new(pairs: &[(&'static str, &'static str)]) -> Self {
            FakeEnv(pairs.iter().copied().collect())
        }
    }

    impl Env for FakeEnv {
        fn get(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|v| v.to_string())
        }
    }

    /// Records what it was asked, so a test can assert herdr was never called.
    struct FakeHerdr {
        response: Option<String>,
        calls: Vec<String>,
    }

    impl FakeHerdr {
        fn silent() -> Self {
            FakeHerdr {
                response: None,
                calls: Vec::new(),
            }
        }
        fn answering(response: &str) -> Self {
            FakeHerdr {
                response: Some(response.to_string()),
                calls: Vec::new(),
            }
        }
    }

    impl Runner for FakeHerdr {
        fn run(&mut self, args: &[&str]) -> Result<String> {
            self.calls.push(args.join(" "));
            match &self.response {
                Some(r) => Ok(r.clone()),
                None => Err(anyhow!("herdr unavailable")),
            }
        }
    }

    const PANE_CURRENT: &str = r#"{"id":"cli:pane:current","result":{"pane":{"agent":"claude",
        "agent_session":{"agent":"claude","kind":"id","source":"herdr:claude","value":"abc-123"},
        "pane_id":"w3:p13"},"type":"pane_current"}}"#;

    #[test]
    fn the_flag_wins_over_every_other_source() {
        let env = FakeEnv::new(&[
            ("FEEDR_AGENT", "codex:from-env"),
            ("HERDR_ENV", "1"),
            ("CLAUDE_CODE_SESSION_ID", "from-claude"),
        ]);
        let mut herdr = FakeHerdr::answering(PANE_CURRENT);

        let id = resolve(Some("claude:from-flag"), &env, &mut herdr).unwrap();

        assert_eq!(id.agent.to_string(), "claude:from-flag");
        assert_eq!(id.source, Source::Flag);
        assert!(herdr.calls.is_empty(), "herdr must not be consulted");
    }

    #[test]
    fn feedr_agent_env_is_used_when_no_flag_is_given() {
        let env = FakeEnv::new(&[
            ("FEEDR_AGENT", "codex:from-env"),
            ("HERDR_ENV", "1"),
            ("CLAUDE_CODE_SESSION_ID", "from-claude"),
        ]);
        let mut herdr = FakeHerdr::answering(PANE_CURRENT);

        let id = resolve(None, &env, &mut herdr).unwrap();

        assert_eq!(id.agent.to_string(), "codex:from-env");
        assert_eq!(id.source, Source::Env);
        assert!(herdr.calls.is_empty(), "herdr must not be consulted");
    }

    #[test]
    fn herdr_pane_current_supplies_the_ref_inside_herdr() {
        let env = FakeEnv::new(&[("HERDR_ENV", "1")]);
        let mut herdr = FakeHerdr::answering(PANE_CURRENT);

        let id = resolve(None, &env, &mut herdr).unwrap();

        assert_eq!(id.agent.to_string(), "claude:abc-123");
        assert_eq!(id.source, Source::HerdrPane);
        assert_eq!(herdr.calls, vec!["pane current"]);
    }

    #[test]
    fn herdr_is_not_consulted_outside_herdr() {
        let env = FakeEnv::new(&[("CLAUDE_CODE_SESSION_ID", "sess-9")]);
        let mut herdr = FakeHerdr::answering(PANE_CURRENT);

        let id = resolve(None, &env, &mut herdr).unwrap();

        assert_eq!(id.agent.to_string(), "claude:sess-9");
        assert_eq!(id.source, Source::ClaudeSession);
        assert!(
            herdr.calls.is_empty(),
            "no HERDR_ENV means no socket call to make"
        );
    }

    #[test]
    fn a_failing_herdr_call_falls_through_to_the_claude_session() {
        let env = FakeEnv::new(&[("HERDR_ENV", "1"), ("CLAUDE_CODE_SESSION_ID", "sess-9")]);
        let mut herdr = FakeHerdr::silent();

        let id = resolve(None, &env, &mut herdr).unwrap();

        assert_eq!(id.agent.to_string(), "claude:sess-9");
        assert_eq!(id.source, Source::ClaudeSession);
    }

    #[test]
    fn a_pane_with_no_agent_session_falls_through() {
        let env = FakeEnv::new(&[("HERDR_ENV", "1"), ("CLAUDE_CODE_SESSION_ID", "sess-9")]);
        let mut herdr = FakeHerdr::answering(r#"{"result":{"pane":{"pane_id":"w1:p1"}}}"#);

        let id = resolve(None, &env, &mut herdr).unwrap();

        assert_eq!(id.source, Source::ClaudeSession);
    }

    #[test]
    fn nothing_resolvable_names_every_source_tried() {
        let env = FakeEnv::new(&[]);
        let mut herdr = FakeHerdr::silent();

        let err = resolve(None, &env, &mut herdr).unwrap_err().to_string();

        for source in [
            "--agent",
            "FEEDR_AGENT",
            "herdr pane current",
            "CLAUDE_CODE_SESSION_ID",
        ] {
            assert!(err.contains(source), "error should name {source}: {err}");
        }
    }

    #[test]
    fn a_malformed_flag_is_an_error_not_a_fallthrough() {
        let env = FakeEnv::new(&[("CLAUDE_CODE_SESSION_ID", "sess-9")]);
        let mut herdr = FakeHerdr::silent();

        let err = resolve(Some("no-colon"), &env, &mut herdr)
            .unwrap_err()
            .to_string();

        assert!(err.contains("kind:id"), "should explain the format: {err}");
    }

    #[test]
    fn a_malformed_feedr_agent_is_an_error_not_a_fallthrough() {
        let env = FakeEnv::new(&[
            ("FEEDR_AGENT", "no-colon"),
            ("CLAUDE_CODE_SESSION_ID", "sess-9"),
        ]);
        let mut herdr = FakeHerdr::silent();

        let err = resolve(None, &env, &mut herdr).unwrap_err().to_string();

        assert!(err.contains("FEEDR_AGENT"), "should name the source: {err}");
    }
}
