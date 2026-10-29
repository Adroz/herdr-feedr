## Agent skills

### Issue tracker

GitHub Issues on `Adroz/herdr-feedr` via the `gh` CLI; wayfinder maps use `wayfinder:*` labels with sub-issues and native blocking. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` and `docs/adr/` at the repo root. See `docs/agents/domain.md`.

## Project docs

- **Product spec:** `docs/SPEC.md` is the source of truth for behaviour. When a change alters behaviour, update the matching §.
- **herdr calls:** read `docs/herdr-api.md` before touching the herdr CLI or socket; several calls are destructive or steal focus.
- **Research:** `docs/research/` holds the cited findings behind past decisions (selection/copy, auto-close, pane placement, agent visibility, skill delivery, test inventory).

## Checks

`scripts/check.sh` runs what CI runs (fmt, clippy, tests); `--fast` skips tests. Enable the pre-commit hook per clone with `git config core.hooksPath .githooks`.
