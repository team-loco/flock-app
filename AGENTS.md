# Flock agent guidelines

Flock is a native macOS app (Rust, gpui) for watching many Kubernetes clusters at once, with Flux
built in and Loco support behind the `loco` cargo feature. Read `docs/PLAN.md` for the architecture,
`docs/decisions.md` for settled questions, and `docs/TASKS.md` for who owns what.

## Before you start

- Read `docs/TASKS.md` and `git log --oneline -20`. If your unit is already done or claimed, stop
  and report.
- Load `.claude/skills/code-quality/SKILL.md`. It is the definition of done.
- Find files before reading them (`rg --files | rg <name>`); names in old notes go stale.

## Commands

Every tool is pinned in `mise.toml`. `mise run <task>`:

- `check` runs everything a PR needs: fmt, clippy (default and `--features loco`), nextest, typos, deny
- `run` / `demo` / `run:loco` start the app (live / sample data / with Loco)
- `shot -- <screen> [dark|light]` renders a demo screen headlessly to `target/shots/<screen>-<appearance>.png`
  (1440×900 @2x; `FLOCK_FEATURES=loco` for Loco screens; `--state loading|connecting|empty|error` for
  page states; `--primary` / `--secondary <preset|#hex>` for highlight colours). No window opens.
- UI interaction tests live in `apps/flock-shots/tests/ui.rs`: `flock_shots::Ui::open(Screen::…)` builds the
  shell on demo data headlessly, then `press("cmd-k")`, `type_text(..)`, `click("<debug selector>")` (also `double_click`,
  `right_click`, `hover`, `drag(selector, offset)`),
  `state()` and `save_png(..)`. Kit buttons, menu items (`<menu>-item-<ix>`) and table rows
  (`<table>-row-<ix>`) carry debug selectors named after their element ids.
- Live UI journeys live in `apps/flock-shots/tests/journeys.rs` (`FLOCK_E2E=1`, run by `e2e:test`):
  `flock_shots::Ui::live(screen, preset, factory)` drives the same shell on a `LiveBackend` over a
  temporary kubeconfig holding only `kind-fleet-a`, `kind-fleet-b` and `kind-flock-loco` (real Loco), with `wait_until(..)` and
  `pause(..)` for real time. The ignored `live_pages_render_for_review` renders every live page
  dark, light and with non-default highlights into `target/qa/`.
- `bench:ui` runs the `#[gpui::bench]` UI responsiveness benchmarks in `apps/flock-bench` (`-- --test` runs
  each once; a Criterion filter such as `-- 'Snapshot apply'` picks one group). The package must never
  enable gpui's `test-support`; the task fails if it does.
- `e2e:up` / `e2e:down` create and remove the kind + Flux scenario clusters (fleet-a, fleet-b, and flock-loco with real Loco)
- `e2e:test` brings them up, verifies, and runs every `FLOCK_E2E` test (flags: `--skip-up`, `--no-teardown`)

## Verifying the UI

- Never launch the GUI app or send synthetic input to it (osascript, cliclick, `screencapture`) to verify
  a change. It takes focus from whoever is using the Mac. Use `mise run shot` and the `Ui` test helper,
  which render without any window. `run` / `demo` are for people only.

## Principles

- Every logic change has a test that fails before it and passes after it.
- Simplicity first: no features, abstractions or configurability nobody asked for.
- Evidence over assumptions: reproduce, then fix the root cause.
- Fail fast on impossible states; validate input at the boundary (kubeconfig, API responses).
- Clean as you go in the files you touch; remove what your change leaves unused.

## Code style

- No code comments. Put reasoning in the commit message or PR body.
- No magic numbers: tunables are fields on a config struct, the rest are named `const`s at the top
  of the file. All visual values (colours, sizes, radii, row heights) come from `flock-ui`'s theme
  module, never literals in a screen.
- One source per value. Versions live in `mise.toml` / `Cargo.toml` only.
- Errors: `thiserror` enums per crate; `anyhow` only in `apps/`. No `unwrap`/`expect` on data
  from a cluster, a file or the user.
- Never group `match` arms with `|` to share a body when the arms mean different things.
- Name intermediate results instead of nesting calls inside calls or struct literals.
- The UI depends only on `flock-core`. Never import `kube`, `flock-kube` or `flock-flux` from
  `flock-ui`.
- Loco code is gated: `#[cfg(feature = "loco")]` on modules, fields, imports and tests.

## Design fidelity

`design/` is the spec: `design/HANDOFF.md`, `design/screens/*.dc.html` (v2, authoritative) and
`design/v1/*.dc.html` (content for screens v2 does not draw, restyled to v2 rules). The HTML
`<script>` blocks hold the sample data. Match layout, copy, sizes and tokens exactly. The product
name is Flock; replace "Shoal" and "Fleet" everywhere.

## Git

- Work in your own worktree on your own branch; never switch branches in the main checkout.
- Commit explicit paths (`git commit -- <paths>`), never `git add -A` blindly.
- One unit = one small PR. Rebase on `origin/main` before opening it.
- Commit and PR text is for strangers: what changed and why, nothing conversational.
