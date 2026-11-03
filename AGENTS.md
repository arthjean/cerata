# Cerata

Native Markdown editor with live preview, in Rust and GPUI, ported from Writer Computer frozen at commit `79b744c`. `PLAN.md` holds the port decisions, `docs/adr/` the decided rules, and `tasks/prd-*.md` (local, gitignored) the epics (`EP-NNN`) and stories (`US-NNN`).

## Boundaries

- Treat the Writer Computer checkout (`WRITER_DIR` in `scripts/oracle/oracle.ts`) and the Zed sources (any local clone, `~/.cargo/git/checkouts/zed-*`) as read-only. Cerata consumes GPUI at the rev pinned in `Cargo.toml`, without a fork; a GPUI fix goes upstream.
- Edit `PLAN.md` only on Arthur's explicit decision.
- Leave decided ADRs as written. A rule change gets a new ADR that names the one it amends, as ADR 0007 and 0008 amend ADR 0006.
- Product constraints that only Arthur changes: no telemetry, no network access beyond the `https_only` image fetch, no webview, and no JavaScript engine other than the isolated QuickJS renderer of ADR 0005.
- Code ported from Writer names its origin in the module doc (`Ported from Writer <file>`), as `NOTICE` requires under GPLv3 section 5a. Obsidian is observed for behavior only: copy none of its code. Writer's name, logo and content stay out of Cerata.
- Before running the real app, back up the `cerata` config and data folders (`%APPDATA%\cerata` on Windows) and restore them afterwards: the app writes settings, recents and sessions there. Open a copy of `crates/cerata/examples/fixtures/` or a demo vault, never personal notes.

## Checks

CI (`.github/workflows/ci.yml`) runs these four on macOS, Windows and Ubuntu. Run all of them before a commit:

```sh
bash scripts/check-core-deps.sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

While iterating, test the affected crate (`cargo test -p cerata_markdown`). `cargo run -p cerata` launches the app.

- The `cerata_markdown` oracle tests compare against snapshots that Writer's own code produces. Regenerate them with `scripts/oracle/oracle.ts` (usage in its header), never by hand. Offsets are UTF-8 bytes.
- A performance figure cited in an ADR, story or commit comes from a reference machine, with its command, per `docs/benchmarks.md`.
- GPUI headless screenshots exist only on macOS and Linux. On Windows, prove a visual change in the real app driven by real OS input, and keep the capture.

## Architecture

- `cerata_core`, `cerata_markdown` and `cerata_render` stay free of `gpui*`, so they test without a window and can back a future Android UI. Put logic there when it needs no window; `crates/cerata` holds the GPUI views. A new core crate joins the list in `scripts/check-core-deps.sh`.
- Every write into a workspace goes through `cerata_core::fs::WorkspaceFs`, which confines paths to the canonical root. Cerata writes only notes, their `{stem}-assets/` folders and `.cerata/config`. Notes stay plain Markdown that Obsidian reads, with no metadata outside the text.
- Record Cerata's own writes in `SelfWriteLog`, as `cerata_core/src/save.rs` does, so the watcher drops their echo instead of reloading or flagging a conflict.
- Background work carries an `EpochToken` (`cerata_core::epoch`) and checks it before publishing, so a result from a previous workspace is dropped.
- Run slow file work (trash, copies, image reads) on the background executor, off the main thread.

## Delivery

- Code, comments, identifiers, UI text and commit messages are in US English. `PLAN.md`, ADRs, `docs/captures/`, `docs/research/` and `docs/benchmarks.md` are in French.
- A UI change runs the `better-interface` skill on the delivered surface and fixes its blocking findings. The report and the `usNNN-<state>.png` captures go in `docs/captures/ep-NNN/README.md`, following `docs/captures/ep-007/`.
- Commits read `type(scope): summary`, with the epic or ADR in parentheses when there is one. Captures land in their own `docs(captures)` commit.
- End a run with the checks run and their result, the manual proof and its capture paths, and what remains unproven.
