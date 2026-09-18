use crate::config;
use crate::feed::ops::{self, Authority, Zone};
use crate::feed::parse::parse;
use crate::feed::write;
use crate::feed::AgentRef;
use crate::feed::{Node, State};
use crate::identity;
use crate::skill;
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "feedr", about = "The feed rack for your herd")]
pub struct Cli {
    /// Feed file (overrides FEEDR_FEED and config)
    #[arg(long, global = true)]
    file: Option<PathBuf>,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List active items
    List,
    /// Show one item with its body
    Show { item: String },
    /// Claim an item: [~] + @agent tag
    Claim {
        item: String,
        /// Claiming agent as kind:session-id, e.g. claude:0198f3ab.
        /// Resolved from the environment when omitted (see `whoami`).
        #[arg(long)]
        agent: Option<String>,
    },
    /// Release a claim: clear the @agent tag and put the item back to [ ]
    Unclaim {
        item: String,
        /// Assert human authority — release someone else's claim (the sidebar
        /// and you use this; agents release only their own).
        #[arg(long)]
        as_human: bool,
    },
    /// Print the agent ref this session claims as, and where it came from
    Whoami,
    /// Add an item
    Add {
        title: String,
        /// Body/context lines
        #[arg(long)]
        body: Vec<String>,
        /// Put it in the reserved "## Agent" section (agent-initiated work)
        #[arg(long)]
        agent_owned: bool,
        /// Target a named section, creating it if missing (reserved: Agent, Done, Feed)
        #[arg(long, conflicts_with = "agent_owned")]
        section: Option<String>,
    },
    /// Mark an item awaiting review: [?]
    Review {
        item: String,
        /// Evidence line appended to the item body; repeatable. Required —
        /// a [?] without its reason is what review exists to prevent.
        #[arg(long, required = true)]
        note: Vec<String>,
    },
    /// Close an item: [x]
    Done {
        item: String,
        /// Assert human authority (the sidebar and you use this; agents must not)
        #[arg(long)]
        as_human: bool,
        /// Evidence line appended to the item body; repeatable.
        #[arg(long)]
        note: Vec<String>,
    },
    /// Archive human [x] items under "# Done"; delete agent [x] items
    Sweep,
    /// Manage this plugin's agent skill (its installed copies)
    Skill {
        #[command(subcommand)]
        command: SkillCmd,
    },
    /// Launch the sidebar TUI in this terminal
    Sidebar {
        /// Dock a sidebar pane into herdr (idempotent open-or-focus), then exit
        #[arg(long)]
        dock: bool,
    },
    /// Internal: herdr-plugin.toml's `tab.created` [[events]] hook entry
    /// point (Plan 3 auto-dock). Run by scripts/on-tab-created.sh, not
    /// meant for interactive use.
    #[command(hide = true)]
    AutoDockHook {
        /// The tab that was just created. Defaults to $HERDR_TAB_ID (what
        /// herdr sets in an event hook's environment) when omitted.
        #[arg(long)]
        tab_id: Option<String>,
    },
}

#[derive(Subcommand)]
enum SkillCmd {
    /// Copy the bundled skill into the agent skill directories
    Install {
        /// Refresh copies that already exist; never create one. What the
        /// plugin build step runs, so installing the plugin doesn't write
        /// into $HOME unasked.
        #[arg(long)]
        refresh_only: bool,
    },
    /// Show where the skill is installed and whether it matches this binary
    Status,
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("cannot find your home directory ($HOME is unset)"))
}

fn run_skill(command: &SkillCmd) -> Result<()> {
    let home = home_dir()?;
    match command {
        SkillCmd::Install { refresh_only } => {
            for action in skill::install(&home, *refresh_only)? {
                match action {
                    skill::Action::Wrote(p) => println!("wrote {}", p.display()),
                    skill::Action::Linked { link, target } => {
                        println!("linked {} -> {}", link.display(), target.display())
                    }
                    skill::Action::LeftAlone(p) => {
                        println!("left {} alone (already a symlink)", p.display())
                    }
                    skill::Action::SkippedNotInstalled(p) => {
                        println!(
                            "skipped {} (not installed; run without --refresh-only)",
                            p.display()
                        )
                    }
                }
            }
        }
        SkillCmd::Status => {
            let rows = skill::status(&home);
            if rows.is_empty() {
                println!("skill not installed (run `feedr skill install`)");
                return Ok(());
            }
            for row in rows {
                let installed = row.installed_version.as_deref().unwrap_or("unstamped");
                let verdict = if row.stale { "STALE" } else { "ok" };
                println!(
                    "{} — skill {installed}, binary {} [{verdict}]",
                    row.path.display(),
                    env!("CARGO_PKG_VERSION")
                );
            }
        }
    }
    Ok(())
}

/// Session ids herdr can currently see, for the `(live)` marker on a claimed
/// item (SPEC §5a). Only consulted inside herdr — outside it there's no socket
/// to answer — and any failure yields an empty set, never an error: liveness
/// is a hint, and `list` must stay usable when herdr isn't there.
fn live_sessions(
    env: &dyn identity::Env,
    herdr: &mut dyn crate::tui::socket::Herdr,
) -> HashSet<String> {
    if env.get("HERDR_ENV").is_none() {
        return HashSet::new();
    }
    herdr
        .list_agents()
        .map(|agents| agents.into_iter().map(|a| a.session_id).collect())
        .unwrap_or_default()
}

/// One item's agent suffix: the tag, plus `(live)` only when herdr confirms
/// that session. Absence asserts nothing — an agent working outside herdr is
/// indistinguishable from one that's gone (SPEC §5a).
fn agent_suffix(agent: Option<&AgentRef>, live: &HashSet<String>) -> String {
    match agent {
        None => String::new(),
        Some(a) if live.contains(&a.id) => format!("  @{a} (live)"),
        Some(a) => format!("  @{a}"),
    }
}

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = config::resolve_feed_path(
        cli.file.clone(),
        std::env::var_os("FEEDR_FEED")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        &config::default_config_dir(),
    );
    if let Cmd::Sidebar { dock } = &cli.command {
        let cfg = config::load_sidebar_config(&config::default_config_dir());
        if *dock {
            let mut runner = crate::tui::dock::HerdrCli::from_env();
            let mut client = crate::tui::socket::UnixSocketClient::from_env();
            let msg = crate::tui::dock::dock(&mut runner, &mut client, &cfg)?;
            println!("{msg}");
            return Ok(());
        }
        return crate::tui::run(path, cfg);
    }
    if let Cmd::AutoDockHook { tab_id } = &cli.command {
        let cfg = config::load_sidebar_config(&config::default_config_dir());
        if !cfg.auto_dock {
            return Ok(()); // feature disabled — silent no-op, not an error
        }
        let tab_id = tab_id
            .clone()
            .or_else(|| std::env::var("HERDR_TAB_ID").ok())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("auto-dock-hook: no tab id (pass --tab-id or set HERDR_TAB_ID)")
            })?;
        let mut runner = crate::tui::dock::HerdrCli::from_env();
        let msg = crate::tui::dock::auto_dock_for_tab(&mut runner, &cfg, &tab_id)?;
        println!("{msg}");
        return Ok(());
    }
    if let Cmd::Skill { command } = &cli.command {
        return run_skill(command);
    }
    if let Cmd::Whoami = &cli.command {
        let mut runner = crate::tui::dock::HerdrCli::from_env();
        let id = identity::resolve(None, &identity::SystemEnv, &mut runner)?;
        println!("{} (from {})", id.agent, id.source.label());
        return Ok(());
    }
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => bail!("cannot read {}: {e}", path.display()),
    };
    let mut doc = parse(&text);

    match cli.command {
        Cmd::List => {
            let mut herdr = crate::tui::socket::UnixSocketClient::from_env();
            let live = live_sessions(&identity::SystemEnv, &mut herdr);
            for (i, node) in doc.nodes.iter().enumerate() {
                match node {
                    Node::Heading { level: 2, text } if ops::zone_of(&doc, i) != Zone::Archive => {
                        println!("{text}:");
                    }
                    Node::Item(it) if ops::zone_of(&doc, i) != Zone::Archive => {
                        let agent = agent_suffix(it.agent.as_ref(), &live);
                        println!("[{}] {}{agent}", it.state.to_char(), it.title);
                    }
                    _ => {}
                }
            }
        }
        Cmd::Show { item } => {
            let mut herdr = crate::tui::socket::UnixSocketClient::from_env();
            let live = live_sessions(&identity::SystemEnv, &mut herdr);
            let i = ops::find(&doc, &item)?;
            if let Node::Item(it) = &doc.nodes[i] {
                let agent = agent_suffix(it.agent.as_ref(), &live);
                println!("[{}] {}{agent}", it.state.to_char(), it.title);
                for b in &it.body {
                    println!("  {b}");
                }
            }
        }
        Cmd::Claim { item, agent } => {
            let mut runner = crate::tui::dock::HerdrCli::from_env();
            // Resolve before locating the item so an unresolvable identity
            // fails without touching the feed.
            let id = identity::resolve(agent.as_deref(), &identity::SystemEnv, &mut runner)?;
            let i = ops::find(&doc, &item)?;
            ops::claim(&mut doc, i, id.agent);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Unclaim { item, as_human } => {
            let by = if as_human {
                ops::Unclaimer::Human
            } else {
                let mut runner = crate::tui::dock::HerdrCli::from_env();
                let id = identity::resolve(None, &identity::SystemEnv, &mut runner)?;
                ops::Unclaimer::Agent(id.agent)
            };
            let i = ops::find(&doc, &item)?;
            ops::unclaim(&mut doc, i, &by)?;
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Add {
            title,
            body,
            agent_owned,
            section,
        } => {
            if let Some(section) = section {
                ops::add_in_section(&mut doc, &title, &body, &section)?;
            } else {
                let zone = if agent_owned {
                    Zone::Agent
                } else {
                    Zone::Human
                };
                ops::add(&mut doc, &title, &body, zone);
            }
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Review { item, note } => {
            let i = ops::find(&doc, &item)?;
            ops::set_state(&mut doc, i, State::Review, Authority::Agent)?;
            // Same document, same save: the note can't be lost by a crash
            // between the state change and the evidence.
            ops::append_note(&mut doc, i, &note);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Done {
            item,
            as_human,
            note,
        } => {
            let i = ops::find(&doc, &item)?;
            let by = if as_human {
                Authority::Human
            } else {
                Authority::Agent
            };
            ops::set_state(&mut doc, i, State::Done, by)?;
            ops::append_note(&mut doc, i, &note);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Sweep => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            ops::sweep(&mut doc, &today);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Sidebar { .. } | Cmd::AutoDockHook { .. } | Cmd::Whoami | Cmd::Skill { .. } => {
            unreachable!()
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::socket::{AgentInfo, AgentStatus, FakeHerdr};

    struct Env(Vec<(&'static str, &'static str)>);
    impl identity::Env for Env {
        fn get(&self, key: &str) -> Option<String> {
            self.0
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_string())
        }
    }

    fn agent(id: &str) -> AgentRef {
        AgentRef::parse(&format!("claude:{id}")).unwrap()
    }

    #[test]
    fn a_confirmed_session_is_marked_live() {
        let live: HashSet<String> = ["sess-1".to_string()].into_iter().collect();
        assert_eq!(
            agent_suffix(Some(&agent("sess-1")), &live),
            "  @claude:sess-1 (live)"
        );
    }

    #[test]
    fn an_unconfirmed_session_gets_the_tag_but_no_claim_about_it() {
        let live: HashSet<String> = ["sess-1".to_string()].into_iter().collect();
        assert_eq!(agent_suffix(Some(&agent("gone")), &live), "  @claude:gone");
    }

    #[test]
    fn an_unclaimed_item_has_no_suffix() {
        assert_eq!(agent_suffix(None, &HashSet::new()), "");
    }

    #[test]
    fn outside_herdr_no_socket_call_is_made() {
        let mut herdr = FakeHerdr {
            agents: vec![AgentInfo {
                pane_id: "w1:p1".into(),
                kind: "claude".into(),
                session_id: "sess-1".into(),
                status: AgentStatus::Working,
            }],
            ..Default::default()
        };

        let live = live_sessions(&Env(vec![]), &mut herdr);

        assert!(live.is_empty(), "no HERDR_ENV means no socket to ask");
    }

    #[test]
    fn inside_herdr_the_live_set_comes_from_the_socket() {
        let mut herdr = FakeHerdr {
            agents: vec![AgentInfo {
                pane_id: "w1:p1".into(),
                kind: "claude".into(),
                session_id: "sess-1".into(),
                status: AgentStatus::Working,
            }],
            ..Default::default()
        };

        let live = live_sessions(&Env(vec![("HERDR_ENV", "1")]), &mut herdr);

        assert!(live.contains("sess-1"));
    }

    #[test]
    fn a_socket_failure_is_silently_no_liveness() {
        let mut herdr = FakeHerdr {
            fail: true,
            ..Default::default()
        };

        let live = live_sessions(&Env(vec![("HERDR_ENV", "1")]), &mut herdr);

        assert!(live.is_empty(), "liveness is a hint; list must still work");
    }
}
