---
name: herdr-feedr
description: Use when working items from the herdr-feedr feed — the shared markdown to-do list in the feedr sidebar. Covers seeing what's on the feed, claiming an item so it links back to this session, adding follow-up work, and finishing an item with evidence. Use whenever the human refers to "the feed", "the rack", "the sidebar list", or asks you to pick up / work / close a to-do item.
---

# herdr-feedr

> Skill version 0.1.0 — requires feedr CLI >= 0.1.0. Run `feedr skill status` if commands here don't match the binary.

The feed is one markdown file shared by a human and their agents. It's rendered live in a
sidebar pane; you reach it through the `feedr` CLI.

## The golden rule

**Never edit the feed file directly.** Not with a text editor, not with `sed`, not by
rewriting it wholesale. Every read and write goes through `feedr`.

The file is written by several parties at once — you, the sidebar, the human, other agents.
`feedr` reads the file fresh, applies one change, and renames a temp file over the original,
so interleaved edits all survive. A direct edit throws away whatever landed between your read
and your write.

## Who you are

```sh
feedr whoami
```

Prints the agent ref you'll claim as (`kind:session-id`) and where it came from. It resolves
from `--agent`, then `FEEDR_AGENT`, then `herdr pane current`, then `CLAUDE_CODE_SESSION_ID`.

If it fails, **stop and ask the human** — don't work around it. The ref is what links a feed
item back to this session, so an untagged claim is a dead link the sidebar can't jump from.

## Getting work

See what's there:

```sh
feedr list
feedr show auth
```

A claimed item's tag reads `@claude:abc (live)` when herdr can currently see that session. **No marker means "not visible", not "dead"** — an agent working outside herdr looks identical to one that's gone, so treat an unmarked claim as taken unless the human says otherwise.

`show` takes any unambiguous substring of the title and prints the item's body — the context
the human left for whoever picks it up. Read it before starting.

Then take the item:

```sh
feedr claim auth
```

That sets `[~]` and stamps your ref. Claim **before** you start working, not after — the stamp
is what tells the human (and other agents) the item is taken.

**If you arrive without a specific task** — the human just said "grab something off the feed" —
list the open items, say which one you propose, and **ask**. Never auto-grab.

## While you work

Work that you discover and own goes on the feed too:

```sh
feedr add "Backfill the migration test" --agent-owned --body "Found while fixing auth; not in scope here."
```

`--agent-owned` files it under the reserved `## Agent` section, marking it as yours rather than
the human's. Drop the flag only when you're recording something on the human's behalf at their
request.

## Giving an item back

If you can't finish what you claimed — wrong agent for the job, blocked on something outside
your reach, the human redirected you — release it rather than leaving it claimed:

```sh
feedr unclaim auth
```

That clears your tag and puts the item back to `[ ]`, so it reads as takeable again. A claim you
walk away from silently is worse than no claim: it tells everyone the work is in hand when it
isn't.

You may release **only items you claimed yourself** — releasing another agent's claim fails, and
`--as-human` is the human's flag, not yours. Once you've handed work back with `review`, the tag
stops being a lock and becomes the record of who did the work, so there's nothing left to release.

## Finishing

Who may close an item depends on who created it.

**Human-created items — you never close.** Move them to awaiting-review and say what you did:

```sh
feedr review auth --note "Fixed the redirect guard in a1b2c3; added a regression test."
```

`--note` is required and repeatable. It's appended to the item body in the same write as the
state change, so the human sees the evidence next to the item. The item lands on `[?]` and the
human decides.

**Agent-created items — your own — you close:**

```sh
feedr done "Backfill the migration test" --note "Landed in d4e5f6."
```

`feedr done` has an `--as-human` flag. It is for the human and the sidebar. **Never pass it.**
Attempting to close a human-created item without it fails, and that failure is the rule
working, not an obstacle to route around.

## Hands off

- `feedr sweep` archives completed items. The human runs it. You don't.
- The `# Done` region at the bottom of the file is an archive. Don't claim from it, don't edit it.
- `feedr sidebar` launches the human's TUI pane. You have no reason to run it.
- `feedr skill install` / `feedr skill status` manage this skill file's installed copies —
  the human's business, not part of working the feed.

## A worked example

The human says "take the auth thing off the feed and fix it".

```sh
feedr whoami                  # claude:2f62ad27-… (from CLAUDE_CODE_SESSION_ID)
feedr list                    # find the item
feedr show auth               # read the context the human left
feedr claim auth              # [~] + your ref — before any code
# ... do the work ...
feedr review auth --note "Redirect guard fixed in a1b2c3; regression test added."
```

The item now reads `[?]` in the sidebar with your note under it, linked to this session. The
human takes it from there.
