---
name: reload-feedr
description: Use when feedr sidebar code has changed and the running sidebars in herdr should pick it up — "reload feedr", "restart the sidebar", "reload the tool in all tabs", or open sidebars still showing old behavior after a build.
---

# Reload feedr

herdr never hot-reloads a pane's process: every open feedr sidebar keeps running the binary it started with. `scripts/reload-feedr.sh` rebuilds the release binary, then replaces the sidebar in every tab of every workspace in place: the new sidebar takes the old one's exact slot, and no other pane moves.

## Run it

```bash
bash scripts/reload-feedr.sh --dry-run   # list the sidebars it would reload
bash scripts/reload-feedr.sh             # cargo build --release, then reload all
```

| Flag | Effect |
|---|---|
| `--no-build` | skip `cargo build --release` |
| `--tab <tab_id>` | only that tab (repeatable). Tab ids come from `herdr tab list` / `herdr pane current` |
| `--dry-run` | print the plan, touch nothing |

It needs `HERDR_ENV=1`, so run it from inside herdr. A failed build stops the script before it touches any pane.

## Reading the output

One line per sidebar (`ok`, `skip` or `FAIL`), then a summary. The exit code is non-zero if any sidebar failed.

- `ok … (width not fitted: …)`: the sidebar was reloaded but kept its old width. Usually it shares no side-by-side split with anything.
- `ok … (re-docked)`: the in-place swap failed, so the script closed the sidebar and docked a fresh one at the left edge instead.
- `skip … only pane`: the in-place swap failed and the sidebar is alone in its tab. The script left it alone, because closing it would close the tab.
- `FAIL … re-dock failed`: that tab now has no sidebar. Fix the error, then rerun with `--tab <id>`.

## What the user should expect

- Every sidebar is set to exactly the configured width: its `width` share of the tab, capped at `max_width` (46 columns by default). Panes next to it keep their widths. The difference comes out of the panes on the other side of the tab, and a width someone dragged by hand is not kept.
- Collapsed state and scroll position reset to the defaults.
- The user's view and focus are restored when it finishes: each workspace goes back to its active tab, and the work pane keeps focus rather than the sidebar. The screen may flicker while it runs.
- Report the summary line. If anything was skipped or failed, list the tab ids.
