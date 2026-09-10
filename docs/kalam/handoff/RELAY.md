# Relaying between the two agents — the owner's checklist

Two AI agents, two repositories, one person carrying messages. This
page is the whole procedure. Nothing here needs understanding of the
code; it needs exact copy-paste.

## The one rule

**Copy the whole block, never a summary of it.** A compiler error with
one line missing is unfixable from the other side. If a message is too
long for the chat, paste it into a file in that repo and commit it
(`docs/engine-handoff/last-report.txt` in calibre-alt,
`docs/kalam/handoff/last-reply.txt` in kalam-engine) and say "see the
file".

## Setup — do once

In kalam-engine (this repo), on a machine with the repo checked out:

```sh
git pull
sh docs/kalam/handoff/make-bundle.sh
```

It prints the copy command. Run it, then in calibre-alt:

```sh
git add docs/engine-handoff && git commit -m "engine handoff bundle" && git push
```

Then tell the calibre-alt agent, in its chat, exactly this:

> Read `docs/engine-handoff/README.md` and follow it. Start with
> INTEGRATION.md step 0. Report using the KALAM REPORT block.

## The loop

1. The calibre-alt agent sends a `=== KALAM REPORT ===` block.
2. Paste the **entire block** to the kalam-engine agent. No commentary
   needed — the block says everything.
3. The kalam-engine agent answers with an `=== ENGINE REPLY ===` block.
4. Paste the **entire block** to the calibre-alt agent.
5. Repeat.

When a reply says **"engine updated to <commit>; regenerate bundle"**:

```sh
# in kalam-engine
git pull && sh docs/kalam/handoff/make-bundle.sh
# copy as printed, then in calibre-alt
git add docs/engine-handoff && git commit -m "engine handoff bundle: <commit>" && git push
```

and tell the calibre-alt agent: "bundle regenerated at <commit>, continue".

## When something is not a report

- The calibre-alt agent asks *you* a question (a preference, a design
  choice): answer it yourself if you have an opinion; otherwise
  forward it as-is to the kalam-engine agent and relay the answer.
- The build works but the app misbehaves (wrong page, missing
  highlight, slow): describe what you saw in your own words to the
  calibre-alt agent first — it has the app's code and can check. It
  will put it in a report if the engine is involved.
- Either agent goes in circles (same error twice): tell the other one
  "the last reply did not fix it; here is the report again" and paste
  it. The engine side is expected to change the widget rather than
  suggest a third workaround.

## What each side can and cannot see

| | kalam-engine agent | calibre-alt agent |
|---|---|---|
| engine source | yes | **no** — only the API reference |
| Kalam source | **no** — only what the recipe remembers | yes |
| your machine's build output | **no** | **no** |
| the other agent's chat | **no** | **no** |

That last row is why the report block exists: everything that matters
must be *inside* it.
