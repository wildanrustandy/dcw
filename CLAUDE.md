# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What dcw is

A single Rust binary (~4.3k lines, no async runtime) that gives each HMX feature branch a
fully isolated dev environment: its own git worktree, port, Postgres clone, Redis namespace,
Django server, and Celery worker. It is a **lifecycle manager, not an application** — every
real workload belongs to external tools it shells out to (`git`, `createdb`/`psql`/`dropdb`,
`lsof`, `curl`/`wget`, and the project's `hmx` and `celery` binaries from the venv).

This matters for how you optimize it: dcw's own memory is a few MB and it exits once it has
spawned things. A running worktree's ~1.6 GB RSS is the Django server and Celery worker
(Python), reached only by changing *how dcw spawns them*, never by changing Rust code.

`README.md` is written in Indonesian and is the user-facing reference; code comments are English.

## Commands

```sh
cargo build --release           # -> target/release/dcw
cargo test                      # 16 tests, <1s
cargo test progress::tests::bar_fills_proportionally   # single test, module::tests::name
cargo clippy --all-targets      # expected clean
```

Install what you built locally: `./target/release/dcw init --yes --bin ~/.local/bin`

## Release

`.github/workflows/release.yml` runs on any `v*` tag and publishes **three** assets:
`dcw-linux-x86_64`, `dcw-macos-aarch64`, `dcw-macos-x86_64`. All three must be present on a
release: `dcw update` resolves `dcw-<os>-<arch>` from the `latest` release URL, so a missing
asset 404s every user on that platform. Asset names must keep matching
`std::env::consts::{OS, ARCH}`, which `update_url()` in `src/main.rs` and the `uname` mapping
in `install.sh` both reproduce — the three spellings have to stay in sync.

Bump `version` in `Cargo.toml` before tagging; `dcw --version` and the release tag are
expected to agree.

**Release notes live in `CHANGELOG.md` and are `include_str!`d into the binary**
(`src/changelog.rs`). This is load-bearing, not cosmetic: `dcw update` prints notes by
running `changelog <old-version>` on the binary it just *downloaded*, because the notes
compiled into the running binary predate the release being installed — only the new binary
can describe itself. So a release that ships without a `## <version>` section installs
silently. Add the section at the top of `CHANGELOG.md` with the same number as the
`Cargo.toml` bump (no `v` prefix), then tag. GitHub release bodies are generated empty by
the workflow; the binary's embedded copy is the source of truth.

## Architecture

`main.rs` is a clap dispatch table plus self-update (`update()`) and `celery status`; logic
lives in the modules.

**Identity is the worktree directory name, not the branch.** A worktree can be `git switch`ed
to another branch at any time while keeping its port, log, and worker, so `find_target()`
(`commands.rs`) resolves a user argument by worktree name first, then by checked-out branch as
a legacy alias. This is why every subcommand is documented as taking "worktree (its directory);
a branch name resolves to its worktree".

**The base branch is not a worktree.** It runs in `$REPO` itself and canonicalizes to
`cfg.base`. `Config::dir_for()` encodes this — it returns `repo` for the base branch and
`wt_dir/<name>` otherwise. Anything that resolves a path must go through it, not joins.

**Two entry paths, deliberately separate.** `create()` (`new`) only builds worktrees that do
not exist; `start()`/`bring_up()` only runs ones that do. `create_one()` handles both the
single-target path (owns its own `Progress` block, `Option<Progress>` = `Some`) and the
batch path (the caller owns the block and passes its row in through the `batch` tuple).

**Concurrency.** `create()` runs a fixed worker pool of `MAX_PARALLEL_NEW` (3) threads pulling
from an `AtomicUsize` index — one fetch and one start-point resolution for the whole batch,
since those are identical for every target. `abort_pick()` likewise spawns one thread per
target. Both use `std::thread::scope` so workers borrow `cfg` directly. The cap is justified by
measurement (recorded in the doc comment): checkout and `createdb -T` are disk-bound at ~94%
utilisation, so extra parallelism buys ~16% while costing RAM and disk.

**Terminal output.** `progress.rs` is a multi-row docker-pull-style block on stderr, driven by
a redraw thread that moves the cursor up over the previous render. It is a **no-op when stderr
is not a tty** (`Progress::start()` checks `IsTerminal`) and every `TaskHandle` method falls
back to plain lines — so piped and scripted runs must keep printing the same information they
always did. Percentages only appear where a real total exists (checkout, from
`ls-tree` vs files actually on disk); `git fetch` and `createdb -T` have no machine-readable
progress and show elapsed seconds instead — do not fabricate a bar for them.

**Process lifecycle.** `process::spawn_detached()` sets `setsid()` in `pre_exec`, so each
child is its own session and group leader — meaning **PGID == PID**, which is what lets
`stop_group()` use `kill(-pid)` to catch the forked Celery children that would otherwise
survive their parent. Logs are append-only, stdout and stderr both redirected into the same fd.

**State lives outside the repo**: `~/.dev-captain/` holds `ports.json` (the port ↔ branch ↔
pid registry), `celery/*.pid`, `logs/`, and `.lock/`. `registry.rs` writes it atomically
(temp file + rename, with a per-call sequence number so concurrent saves in one process do not
share a temp path) and serializes writers with `flock`, which the kernel releases on exit —
a killed dcw cannot leave a stuck lock. A corrupt `ports.json` is reported and left untouched
rather than read as empty, because the next save would otherwise erase every tracked port.

**Config precedence** is env → `~/.config/dcw/config` → built-in default (`config.rs::resolve`;
an explicitly empty env var is honored, not treated as unset). The config file is parsed
loosely on purpose: it accepts both `KEY=VALUE` and dcw's historical shell form
`: "${KEY:=VALUE}"`, so `parse_assignment`/`trailing_identifier`/`clean_value` exist to handle
it. Base branch detection and preview-domain detection both fall back through several sources
before giving up.

**Shell integration.** `dcw jump` and `dcw new` must `cd` the *calling* shell, which a child
process cannot do, so `setup.rs::install_shell_integration` writes a `dcw()` function into the
login shell's rc file. `jump` prints the path on stdout (captured by `$(...)`); `new` passes it
through `$DCW_CD_FILE` instead, because with a picker a plain stdout handoff would be
ambiguous. Only single-target `new` writes the cd file — with several targets there is no one
directory to enter. Menu output in `picker.rs` likewise goes to `/dev/tty`, never stdout.

## Testing notes

Tests are colocated `#[cfg(test)]` modules, concentrated in `progress.rs`, `registry.rs`,
`setup.rs`, and `process.rs` (the pure and hard-to-observe logic). `commands.rs`, `db.rs`,
`celery.rs`, and `git.rs` have none.

`cargo test`'s stderr is not a tty, so `Progress::start()` takes its non-tty branch and stores
no state. Tests that need to observe rows construct `Shared { tasks, drawn, tty: true }`
directly (see `concurrent_rows_keep_their_own_state`) rather than going through `start()`.

## Working on this repo

- Conventional-commit subjects (`feat:`, `fix:`, `perf:`, `ci:`); work lands through a PR.
- The README documents behavior in detail — update it when a command's contract changes,
  since it is what users actually read.
- Verify claims about performance or memory by measuring. The doc comments in `commands.rs`
  and `progress.rs` record concrete numbers (timings, `%util`, disk/RAM cost); keep that
  standard rather than asserting a guess.