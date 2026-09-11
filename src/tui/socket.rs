use anyhow::Result;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatus {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
}

impl AgentStatus {
    /// `unknown` (and anything unrecognized) must never render as done —
    /// research-doc caveat.
    pub fn parse(s: &str) -> Self {
        match s {
            "idle" => AgentStatus::Idle,
            "working" => AgentStatus::Working,
            "blocked" => AgentStatus::Blocked,
            "done" => AgentStatus::Done,
            _ => AgentStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentInfo {
    pub pane_id: String,
    pub kind: String,
    /// `agent_session.value` — joins to the feed's `@agent(kind:id)` id part.
    pub session_id: String,
    pub status: AgentStatus,
}

/// The entries inside a method result: a bare array, or the first array-valued
/// field of a result object (the research doc pins the entry shape but not the
/// envelope, so read tolerantly).
pub(crate) fn result_entries(result: &Value) -> Vec<&Value> {
    if let Some(a) = result.as_array() {
        return a.iter().collect();
    }
    if let Some(obj) = result.as_object() {
        for v in obj.values() {
            if let Some(a) = v.as_array() {
                return a.iter().collect();
            }
        }
    }
    Vec::new()
}

pub fn parse_agent_list(result: &Value) -> Vec<AgentInfo> {
    result_entries(result)
        .into_iter()
        .filter_map(|v| {
            Some(AgentInfo {
                pane_id: v.get("pane_id")?.as_str()?.to_string(),
                kind: v.get("agent")?.as_str()?.to_string(),
                session_id: v.get("agent_session")?.get("value")?.as_str()?.to_string(),
                status: AgentStatus::parse(
                    v.get("agent_status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown"),
                ),
            })
        })
        .collect()
}

/// Pass-through args that resume a session for an agent kind (spec §3:
/// "kind prefix selects the command"). These ride in `agent.start`'s `args`
/// field, appended to the agent binary the kind selects.
pub fn resume_args(kind: &str, session_id: &str) -> Option<Vec<String>> {
    match kind {
        "claude" => Some(vec!["--resume".into(), session_id.into()]),
        _ => None,
    }
}

/// Human-readable fallback shown in the status line when herdr can't do it.
pub fn resume_hint(agent: &crate::feed::AgentRef) -> String {
    resume_args(&agent.kind, &agent.id)
        .map(|a| format!("{} {}", agent.kind, a.join(" ")))
        .unwrap_or_else(|| agent.to_string())
}

/// Everything the sidebar asks of herdr, behind a trait so tests fake it.
/// Verbs verified against live herdr 0.9.0 (`herdr api schema --json`).
pub trait Herdr {
    fn list_agents(&mut self) -> Result<Vec<AgentInfo>>;
    /// `agent.focus {target}` — jump the herdr UI to the agent's pane.
    fn focus_agent(&mut self, target: &str) -> Result<()>;
    /// `pane.focus {pane_id}` — plain pane focus (used by the dock launcher).
    fn focus_pane(&mut self, pane_id: &str) -> Result<()>;
    /// `tab.create` (TabCreateParams: workspace_id/cwd/env/label/focus — it
    /// spawns a shell pane, never a command). Returns the new pane's id.
    fn create_tab(&mut self, label: &str) -> Result<String>;
    /// `agent.start` (AgentStartParams: {name, kind, pane_id, args[],
    /// timeout_ms}) — start an agent in a pane; `args` pass through to the
    /// agent binary.
    fn agent_start(&mut self, name: &str, kind: &str, pane_id: &str, args: &[String])
        -> Result<()>;

    /// Resume a session in a fresh tab (spec §3: @agent click with the pane
    /// gone): create a tab, then start the agent in its pane with the kind's
    /// resume args. `agent.start` succeeding means herdr *detected* the
    /// agent — the item's @agent link is live again and its status glyph
    /// returns on the next resync. Provided so every impl (and the fake)
    /// shares one composition.
    fn open_resume_tab(&mut self, kind: &str, session_id: &str) -> Result<()> {
        let args = resume_args(kind, session_id)
            .ok_or_else(|| anyhow::anyhow!("no resume command for agent kind \"{kind}\""))?;
        let pane_id = self.create_tab(&format!("resume {kind}"))?;
        let name = format!(
            "feedr-resume-{}",
            session_id.chars().take(8).collect::<String>()
        );
        self.agent_start(&name, kind, &pane_id, &args)
    }
}

#[cfg(test)]
#[derive(Default, Clone)]
pub struct FakeHerdr {
    pub fail: bool,
    pub agents: Vec<AgentInfo>,
    /// Shared so tests can keep a clone and inspect calls after boxing.
    pub log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
}

#[cfg(test)]
impl Herdr for FakeHerdr {
    fn list_agents(&mut self) -> Result<Vec<AgentInfo>> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        Ok(self.agents.clone())
    }
    fn focus_agent(&mut self, target: &str) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!("focus_agent {target}"));
        Ok(())
    }
    fn focus_pane(&mut self, pane_id: &str) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!("focus_pane {pane_id}"));
        Ok(())
    }
    fn create_tab(&mut self, label: &str) -> Result<String> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!("create_tab {label}"));
        Ok("w9:p9".into())
    }
    fn agent_start(
        &mut self,
        name: &str,
        kind: &str,
        pane_id: &str,
        args: &[String],
    ) -> Result<()> {
        if self.fail {
            anyhow::bail!("herdr socket unavailable");
        }
        self.log.borrow_mut().push(format!(
            "agent_start {name} {kind} {pane_id} {}",
            args.join(" ")
        ));
        Ok(())
    }
    // open_resume_tab: provided method — the fake exercises the real
    // composition, so tests see the create_tab + agent_start sequence.
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Entry shape verified live against herdr 0.9.0 in the research doc.
    fn sample_entry() -> serde_json::Value {
        json!({
            "pane_id": "w1:p7", "tab_id": "w1:t5", "workspace_id": "w1",
            "agent": "claude",
            "agent_status": "working",
            "agent_session": {"agent": "claude", "kind": "id", "source": "herdr:claude",
                              "value": "13bb6c2a-1b44-4485-a9c3-e02f5d662dfd"},
            "terminal_title_stripped": "documentation issue 17914",
            "focused": false
        })
    }

    #[test]
    fn parses_agent_list_entries() {
        // Wrapped in an object (as `herdr agent list` result) or a bare array.
        for result in [json!({"agents": [sample_entry()]}), json!([sample_entry()])] {
            let agents = parse_agent_list(&result);
            assert_eq!(
                agents,
                vec![AgentInfo {
                    pane_id: "w1:p7".into(),
                    kind: "claude".into(),
                    session_id: "13bb6c2a-1b44-4485-a9c3-e02f5d662dfd".into(),
                    status: AgentStatus::Working,
                }]
            );
        }
    }

    #[test]
    fn skips_panes_without_agent_session() {
        let result = json!([{"pane_id": "w1:p2", "agent_status": "unknown"}]);
        assert!(parse_agent_list(&result).is_empty());
    }

    #[test]
    fn status_strings_map_and_unknown_is_never_done() {
        assert_eq!(AgentStatus::parse("working"), AgentStatus::Working);
        assert_eq!(AgentStatus::parse("blocked"), AgentStatus::Blocked);
        assert_eq!(AgentStatus::parse("done"), AgentStatus::Done);
        assert_eq!(AgentStatus::parse("idle"), AgentStatus::Idle);
        assert_eq!(AgentStatus::parse("something-new"), AgentStatus::Unknown);
    }

    #[test]
    fn resume_args_per_kind() {
        assert_eq!(
            resume_args("claude", "abc-123"),
            Some(vec!["--resume".into(), "abc-123".into()])
        );
        assert_eq!(resume_args("mystery-agent", "abc"), None);
        let a = crate::feed::AgentRef::parse("claude:abc-123").unwrap();
        assert_eq!(resume_hint(&a), "claude --resume abc-123");
        let b = crate::feed::AgentRef::parse("mystery:zz").unwrap();
        assert_eq!(resume_hint(&b), "mystery:zz");
    }

    #[test]
    fn open_resume_tab_composes_tab_create_and_agent_start() {
        let mut fake = FakeHerdr::default();
        fake.open_resume_tab("claude", "abc-123").unwrap();
        assert_eq!(
            fake.log.borrow().as_slice(),
            [
                "create_tab resume claude",
                "agent_start feedr-resume-abc-123 claude w9:p9 --resume abc-123",
            ]
        );
        // Unknown kinds can't be resumed; failure surfaces before any call:
        assert!(fake.open_resume_tab("mystery", "x").is_err());
        assert_eq!(fake.log.borrow().len(), 2);
    }
}
