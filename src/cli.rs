use crate::config;
use crate::feed::ops::{self, Authority, Zone};
use crate::feed::parse::parse;
use crate::feed::write;
use crate::feed::{AgentRef, Node, State};
use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
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
        /// Claiming agent as kind:session-id, e.g. claude:0198f3ab
        #[arg(long)]
        agent: String,
    },
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
    Review { item: String },
    /// Close an item: [x]
    Done {
        item: String,
        /// Assert human authority (the sidebar and you use this; agents must not)
        #[arg(long)]
        as_human: bool,
    },
    /// Archive human [x] items under "# Done"; delete agent [x] items
    Sweep,
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
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => bail!("cannot read {}: {e}", path.display()),
    };
    let mut doc = parse(&text);

    match cli.command {
        Cmd::List => {
            for (i, node) in doc.nodes.iter().enumerate() {
                match node {
                    Node::Heading { level: 2, text } if ops::zone_of(&doc, i) != Zone::Archive => {
                        println!("{text}:");
                    }
                    Node::Item(it) if ops::zone_of(&doc, i) != Zone::Archive => {
                        let agent = it
                            .agent
                            .as_ref()
                            .map(|a| format!("  @{a}"))
                            .unwrap_or_default();
                        println!("[{}] {}{agent}", it.state.to_char(), it.title);
                    }
                    _ => {}
                }
            }
        }
        Cmd::Show { item } => {
            let i = ops::find(&doc, &item)?;
            if let Node::Item(it) = &doc.nodes[i] {
                println!("[{}] {}", it.state.to_char(), it.title);
                for b in &it.body {
                    println!("  {b}");
                }
            }
        }
        Cmd::Claim { item, agent } => {
            let a = AgentRef::parse(&agent)
                .ok_or_else(|| anyhow::anyhow!("--agent must be kind:id, got \"{agent}\""))?;
            let i = ops::find(&doc, &item)?;
            ops::claim(&mut doc, i, a);
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
        Cmd::Review { item } => {
            let i = ops::find(&doc, &item)?;
            ops::set_state(&mut doc, i, State::Review, Authority::Agent)?;
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Done { item, as_human } => {
            let i = ops::find(&doc, &item)?;
            let by = if as_human {
                Authority::Human
            } else {
                Authority::Agent
            };
            ops::set_state(&mut doc, i, State::Done, by)?;
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Sweep => {
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
            ops::sweep(&mut doc, &today);
            write::save_atomic(&doc, &path)?;
        }
        Cmd::Sidebar { .. } | Cmd::AutoDockHook { .. } => {
            unreachable!()
        }
    }
    Ok(())
}
