# Research: how herdr plugins ship skills to coding agents

**Ticket:** [#4](https://github.com/Adroz/herdr-feedr/issues/4) (part of #1) · **Date:** 2026-09-10
**Question:** herdr-file-viewer ships a `skills/` directory. How do plugin-shipped skills reach
coding agents running in herdr panes — and what delivery mechanism should herdr-feedr's
"pull an item off the feed and work it" skill use?

> Repo has no prior research-notes convention (empty repo); this file establishes
> `docs/research/<topic>.md`.

## TL;DR

**Herdr itself never delivers skills to agents.** Plugin manifest v1 has no skill-related keys,
and `herdr plugin install` only copies the repo into herdr's plugin data dir and reads the
manifest. The `skills/` directory in a plugin repo is inert convention: `skills/<name>/SKILL.md`
with Claude-style frontmatter, delivered by one of three *out-of-band* mechanisms observed in the
ecosystem — (1) the cross-agent `skills` CLI (`npx skills add owner/repo`), which installs into
`~/.agents/skills/<name>/` and symlinks into each agent CLI's skill dir; (2) a plugin-owned
installer command that copies the skill into place and manages drift (memex); (3) a documented
paste-in block for `AGENTS.md`/`CLAUDE.md` as the lowest-common-denominator fallback
(herdr-file-viewer). Feedr should bundle `skills/herdr-feedr/SKILL.md` and deliver it via the
skills-CLI convention plus an AGENTS.md fallback, refreshing copies from its own install/build
step (details and constraints below).

## Sources (primary)

| # | Source | What it establishes |
|---|--------|---------------------|
| S1 | `~/.config/herdr/plugins/github/herdr-file-viewer-c993314e2614/herdr-plugin.toml` (v1.16.0, installed) | Full real-world manifest: **zero** skill keys |
| S2 | Same plugin: `skills/herdr-file-viewer/SKILL.md` | Bundled-skill format (Claude-style `name`/`description` frontmatter) |
| S3 | Same plugin: `docs/usage.md` §"Teach your agent" (lines 97–139), `README.md` line 37 | The plugin's own documented delivery: manual |
| S4 | https://herdr.dev/docs/plugins (fetched 2026-09-10) | Manifest v1 key list; install/update model; no skills support |
| S5 | `herdr --help`, `herdr integration status` (herdr installed locally) | No `skill` command; `integration install <agent>` installs state-report hooks, not skills |
| S6 | `~/.claude/skills/` + `~/.agents/skills/` on this machine + shell history | Observed skills-CLI install: `npx skills add herdrdev/herdr --skill herdr -g` → `~/.agents/skills/herdr/` + symlink `~/.claude/skills/herdr -> ../../.agents/skills/herdr` |
| S7 | GitHub: `nicosuave/memex` README; `jhochenbaum/herdr-hunk-diff` README | Two other registry plugins that ship skills; memex's self-managed installer |
| S8 | `~/.config/herdr/plugins.json` | What herdr actually records about an installed plugin (manifest fields only) |

## Findings

### 1. Herdr has no skill mechanism — confirmed three ways

- **Manifest:** herdr-plugin.toml v1 documents top-level `id`, `name`, `version`,
  `min_herdr_version`, `description`, `platforms` and tables `[[build]]`, `[[startup]]`,
  `[[actions]]`, `[[events]]`, `[[panes]]`, `[[link_handlers]]` — nothing skill-related (S4).
  The file-viewer's real manifest declares only `build`/`panes`/`actions` (S1), and herdr's
  `plugins.json` record of it contains no skill entry (S8).
- **CLI:** `herdr` has no `skill` subcommand (S5). `herdr integration install claude` exists but
  installs an *agent-state hook* (`~/.claude/hooks/herdr-agent-state.sh`) so herdr can track the
  agent — it does not install skills (S5).
- **Docs:** the plugin docs state runtime registration is out of scope for v1; "reinstall from
  GitHub to refresh a managed plugin" is the whole update story (S4).

So when `herdr plugin install owner/repo` clones a plugin into
`~/.config/herdr/plugins/github/<repo>-<hash>/`, a bundled `skills/` directory just sits there.
No agent CLI looks inside herdr's plugin data dir.

### 2. The bundled-skill convention: `skills/<skill-name>/SKILL.md`

The file-viewer ships exactly one skill at `skills/herdr-file-viewer/SKILL.md` with Claude
Code-style YAML frontmatter (`name`, `description`) followed by markdown instructions (S2). The
skill teaches agents the pane-launch incantation
(`herdr plugin pane open --plugin … --entrypoint … --env "HERDR_FILE_VIEWER_OPEN=…"`); its
`docs/usage.md` is explicit that **"Agents do not know this surface by default"** and that without
the skill "a vague 'open it in the file viewer' is only a wish" (S3).

The same layout appears in the other skill-shipping repos checked: `herdrdev/herdr` itself
(`skills/herdr/`), `jhochenbaum/herdr-hunk-diff` (`skills/hunk-herdr-review/`), and
`nicosuave/memex` (`skills/memex-search/`) (S6, S7). Directory name = skill name.

### 3. Three delivery mechanisms observed in the wild

**A. Cross-agent `skills` CLI (herdr's own skill uses this).** This machine's `herdr` skill was
installed with `npx skills add herdrdev/herdr --skill herdr -g` (S6). Result: the canonical copy
lives at `~/.agents/skills/herdr/SKILL.md`; `~/.claude/skills/herdr` is a relative symlink to it.
Other skills on this machine (`find-skills`, `wayfinder`, the mattpocock set) follow the same
pattern: `~/.agents/skills/` is the emerging agent-neutral home, with per-CLI dirs
(`~/.claude/skills/`) symlinked in. The CLI discovers skills by the `skills/<name>/SKILL.md` repo
layout — the same layout the plugins already use, so one repo serves both `herdr plugin install`
and `npx skills add`.

**B. Plugin-owned installer with drift management (memex — the most engineered).** Memex's binary
installs its own skill: `memex skill install --target shared` → one copy at
`~/.agents/skills/memex-search/SKILL.md` for Codex/OpenCode/Pi/OMP; `--target claude` → a
*separate copy* at `~/.claude/skills/memex-search/SKILL.md` (copy, not symlink). It then treats
drift as a first-class problem: searches warn on stderr when the installed skill differs from the
running binary; `memex skill status` shows differing copies; `memex skill update` refreshes them;
binary updates refresh installed copies; "Restart your agent after its skill is updated" (S7).

**C. Documentation-only / AGENTS.md paste-in (herdr-file-viewer, hunk-diff).** The file-viewer's
official instruction: "Use it where your agent runner supports skills, or paste the short block
below into your project's `AGENTS.md` (preferred: every agent reads it) or `CLAUDE.md`" (S3). It
ships a condensed ~30-line block for that purpose. Notably the user on this machine installed the
file-viewer plugin but its skill is **not** installed in any skill dir — evidence that
doc-only delivery frequently doesn't happen.

### 4. Constraints that shape the choice

- **No automatic path exists.** Whatever feedr does, skill delivery is a step *outside*
  `herdr plugin install`. Minimize it to one command and make it re-runnable.
- **Per-agent-CLI differences.** Claude Code reads `~/.claude/skills/` (and project
  `.claude/skills/`); Codex/OpenCode/Pi/OMP read `~/.agents/skills/` (memex targets exactly that
  split, S7); some CLIs support no skill dir at all → the AGENTS.md block is the only universal
  fallback. Symlink support also varies (memex copies rather than symlinks for Claude).
- **Versioning/updates.** Herdr updates a plugin by reinstall (S4). A skill copy installed once
  will drift from the plugin's CLI surface — the file-viewer already shipped a skill/launcher
  divergence bug (#139: skill said pass `--cwd`, launcher never did; S1 header, its CHANGELOG) and
  now CI-tests skill/docs/scripts consistency. Memex's answer is version-stamped copies plus
  status/warn/refresh commands (S7).
- **Symlinking into the plugin checkout is fragile.** The install dir carries a hash suffix
  (`herdr-file-viewer-c993314e2614`); the docs don't guarantee that path across reinstalls, and an
  uninstall would leave dangling symlinks in agent skill dirs. Prefer copies (memex's choice) or
  symlinks into `~/.agents/skills/` only.
- **Skill body must be launch-accurate.** The file-viewer skill's hard-won rules (don't pass
  `--cwd`; the acting pane's cwd governs; treat targets as data not shell source) show the skill
  is the *only* place agents learn the plugin's CLI contract — keep it in the same repo as the
  code and test them against each other.

## Recommendation for herdr-feedr

Bundle the skill in-repo and deliver it via the skills-CLI convention, with an installer-managed
refresh and an AGENTS.md fallback:

1. **Ship `skills/herdr-feedr/SKILL.md`** (Claude-style frontmatter; directory name = skill name).
   One repo layout then serves `herdr plugin install Adroz/herdr-feedr` *and*
   `npx skills add Adroz/herdr-feedr -g`.
2. **Primary install path:** document `npx skills add Adroz/herdr-feedr -g` — lands the canonical
   copy in `~/.agents/skills/herdr-feedr/` and links it into `~/.claude/skills/` (S6 shows this
   working for herdr's own skill). Covers Claude Code + the `~/.agents/skills` CLIs in one step.
3. **Refresh on plugin (re)install:** feedr's `[[build]]` step (or a `feedr skill install`
   command, memex-style, if feedr ships a binary) should copy the bundled skill over any existing
   installed copy, so herdr's reinstall-to-update flow also updates the skill. Copy, don't
   symlink into the hashed plugin dir. Print what it did — writing into the user's home from a
   build step should be announced, and the docs don't sanction or forbid it.
4. **Fallback:** include a short paste-in block for `AGENTS.md`/`CLAUDE.md` in the README
   (file-viewer's pattern) for runners without skill support.
5. **Version-stamp the skill** (plugin version in the SKILL.md body or frontmatter) and add a
   consistency test between the skill's documented commands and the actual CLI/actions — the
   file-viewer's #139 regression is the cautionary tale.
