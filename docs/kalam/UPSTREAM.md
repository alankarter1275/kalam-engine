# Keeping up with Chapbook

We forked Chapbook but want its bug fixes. This is the routine. Once a
month is plenty. Each step is a command you can paste.

## The facts

| | |
|---|---|
| Upstream repo | https://github.com/ophymx/chapbook |
| Imported at | commit `ab14cb78d2f7e63e8e2f7bc066bfc8ee57318cc9`, 2026-09-07 |
| How it was imported | `git merge --allow-unrelated-histories` — the full history is in this repo |
| Last reviewed up to | `ab14cb7` *(update this line every time you finish a review)* |

Because the history was merged rather than copied, Git knows exactly which
upstream commits we already have. That is what makes everything below
one-liners.

## One-time setup (per clone)

```sh
git remote add upstream https://github.com/ophymx/chapbook.git
```

## The monthly review

**1. Fetch what's new.**

```sh
git fetch upstream main
```

**2. List the commits we don't have yet, but only the ones touching crates
we kept.** This ignores everything about Android, iOS, Windows, PDF,
comics, catalogs, sync — i.e. most of the noise.

```sh
git log --format='%h  %ad  %s' --date=short --reverse HEAD..upstream/main -- \
    crates/chapbook-core crates/chapbook-epub crates/chapbook-layout \
    crates/chapbook-paint crates/chapbook-render-tinyskia \
    crates/chapbook-reader crates/chapbook-viewer-gtk tools/chapbook-cli \
    fixtures/epub fixtures/render fixtures/fonts docs
```

Empty output means nothing to do this month. Update the "last reviewed"
line above and stop.

**3. Sort each commit.** Read the message; the author writes long,
explanatory ones. Ask: *is this a fix or improvement to something we
kept?* Three buckets:

| Bucket | Example | Action |
|---|---|---|
| **Take** | "Fix a page break landing inside a heading", "Handle an EPUB whose spine points at a missing file" | cherry-pick it (step 4) |
| **Skip** | Anything about e-ink, Android, iOS, Windows, WASM, PDF, CBZ, OPDS, sync, GPU | nothing |
| **Unsure** | It touches `chapbook-reader` but mentions comics | look at the diff: `git show <hash> --stat`. If it only touches files we deleted, skip. If mixed, cherry-pick and delete the parts that don't apply. |

**4. Bring a commit over.**

```sh
git cherry-pick -x <hash>
```

The `-x` writes "(cherry picked from commit …)" into the message so we
always know where it came from. Three things can happen:

- **Applies cleanly.** Run the gate, commit is already made. Done.
- **Conflict in a file we deleted.** Git says something like
  `CONFLICT (modify/delete)`. That part of the fix is for a crate we
  removed. Resolve with `git rm <that file>` and `git cherry-pick --continue`.
- **Conflict inside a file we kept.** This means we changed that file
  ourselves — which §3 of `PLAN.md` says to avoid, and this is why. Open
  the file, look for the `<<<<<<<` markers, keep both sides' intent, remove
  the markers, `git add` it, `git cherry-pick --continue`. If it is a mess,
  `git cherry-pick --abort` and note the hash in the "deferred" list below
  instead of fighting it.

**5. Run the gate** after each cherry-pick, or after a batch:

```sh
cargo fmt --all --check && cargo clippy --workspace --all-targets && cargo test --workspace
```

If a test fails after a cherry-pick, the fix depended on another upstream
commit you skipped. `git log --oneline <hash>~5..<hash>` shows its
neighbours; usually the missing one is right before it.

**6. Update the "last reviewed" line** at the top of this file and commit.

## Deferred

Upstream commits we wanted but could not take cleanly. Revisit when there
is time; delete the line if it stops mattering.

*(none yet)*

## Things we changed inside inherited files

Kept short on purpose — see `PLAN.md` §3. Every entry here is a future
conflict. Prefix such commits with `kalam:`.

| File | What | Why |
|---|---|---|
| `.github/workflows/ci.yml` | Replaced Chapbook's 9-job matrix with a single Linux job | We ship to one platform |
| `README.md` | Replaced | Describes this fork, not Chapbook |
| `crates/chapbook-reader/src/layout.rs` | Honour `ReadingSettings::publisher_styles` (pass no author sheets when off) | Upstream flag was persisted but never read — a bug. **Candidate to send upstream.** |
| `crates/chapbook-viewer-gtk/src/linux.rs` | `f` (force reader font) and `s` (publisher styles off) toggles | Testing aid; Kalam's adapter sets both permanently |

## If upstream goes quiet or goes a direction we dislike

Nothing breaks. The last version we took keeps working forever; we simply
stop doing the review. That is the point of owning the copy.
