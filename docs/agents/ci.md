# CI

Rules for `.github/workflows`, local actions, CI caches, and the toolchain
bootstrap. Every CI job runs `mise run` tasks, so reproduce failures locally with
the same task. Ghostty's prebuilt-archive cache is described in
[the engine guide](terminal-engines.md#pin-and-native-build).

## Toolchain bootstrap

Keep `verify:toolchain` as a serial preflight before `verify:parallel`. CI uses
`ci:toolchain` to install Rust serially, run that preflight, and retry one clean
installation if either installation or verification fails. Keep Rust and Cargo
tools out of the Mise action install list so an early failure cannot skip this
recovery; install Cargo tools only after the bootstrap succeeds. Mise's CI
cache can restore its Rust install symlink without the corresponding rustup
toolchain, and parallel Cargo invocations then race while materializing it.

On GitHub-hosted macOS runners, set `RUSTUP_HOME` and `CARGO_HOME` under
`/Users/runner/.local/share/mise` and disable the Mise cache. Mise-specific home
variables do not reach nested Cargo tool installs, which can otherwise race in
the image's shared Rustup state. Install `cargo:*` tools serially only after
`verify:toolchain`; a parallel Mise install can publish the Rust tool before its
Cargo component is dispatchable.

Pass `${{ github.token }}` to Pinact as `PINACT_GITHUB_TOKEN` in CI. The Mise
action's token environment is Mise-specific, so Pinact otherwise uses GitHub's
anonymous API limit while verifying action pins.

Every `jdx/mise-action` step pins the same explicit mise `version`. Without
a pin, a restored Mise cache holding an older binary makes the action run
`mise self-update`, whose GitHub API lookup was rate limited (403).
`scripts/ci-toolchain.test.ts` enforces one shared pin, but not release age:
choose a release older than three days by hand. Dependabot does not update
action inputs, so bump every pin together.

`scripts/linux/Dockerfile` pins its own checksummed mise release.

## Jobs and caches

CI disables Mise auto-install so nested tasks retain each job's explicit tool
selection. Its tool cache key hashes `mise.lock` and `rust-toolchain.toml` rather
than task definitions. CI runs format,
Clippy, schema and script checks, and Rust tests sequentially in one job per
platform so they reuse setup and debug artifacts without contending on the Cargo
target-directory lock. Keep desktop smokes separate and serial within their job.

The smoke job separately times Ghostty preparation, compilation, and execution.
Its Cargo cache retains workspace crates, and its additional `.native/ghostty`
cache key hashes the pinned Rust, Cargo, and Ghostty inputs explicitly. Do not
restore rust-cache's automatic Rust environment hash: hosted images can carry
different unrelated toolchains between runs, preventing valid cache restores.
Keep `cache-on-failure` enabled so transient native smoke failures do not discard
a successful compilation before the requested rerun.

Rust caches save only from main (`save-if`); PRs restore main's entries. Never
set a `CARGO*`, `RUST*`, `CC*`, `CXX*`, or `CMAKE*` variable that differs between
PR and main runs: rust-cache hashes those prefixes into the key, so PRs would
stop matching main. The smoke key must name any such workflow value itself.

CI Linux packaging retries once with retained Cargo and Zig caches because
Ghostty's native dependency downloads can fail transiently. A second failure
remains authoritative.

CI installs Zig through `.github/actions/setup-zig`, which reads the version
from `mise.toml` and runs `mlugg/setup-zig`; keep `zig` out of CI's Mise install
lists. Mise's `core:zig` fetches the `.minisig` only from the mirror that served
the tarball, so one rate-limited mirror (HTTP 429) failed the job. The action
falls through to the next mirror and then ziglang.org, but a stalled mirror
holds it for about 14 minutes: three 3-minute attempts, then a slow exit.
Jobs bound the step to 4 minutes and retry once within 6 minutes, which
reshuffles the mirrors; a stall in both attempts fails the job.

This only matters when the tarball cache misses. It always sets
`ZIG_GLOBAL_CACHE_DIR` and `ZIG_LOCAL_CACHE_DIR` to `.zig-cache` in the
checkout, so the wrapper moves them under `RUNNER_TEMP`. Release jobs keep
Mise's Zig install: the action's tarball cache cannot be disabled and a cache
hit skips signature verification, while release installs never use the Actions
cache. Reference local actions as `./.github/actions/...` with a
`zizmor: ignore[self-repository]` comment: actionlint rejects GitHub's `$/`
self-repository syntax.

CI invokes `ci:smoke:build` separately after the workflow's dependency-preparation
step. When a smoke binary gains a native compile-time dependency, prepare it in
that workflow step as well as in the developer-facing smoke task. Keep the
aggregate `ci:smoke:build` targets aligned with the binaries consumed by
`ci:smoke:run`.

Give each independent check its own named step. In jobs that run several,
guard each with `if: ${{ !cancelled() && steps.<setup>.outcome == 'success' }}`,
where `<setup>` is the job's last setup step, as the Checks, Policy, and smoke
jobs do. Without the guard, the first failing step skips the rest and hides
their results. `scripts/ci-steps.test.ts` accepts only the exact guard forms
it lists, because GitHub expressions offer too many ways to skip a step to
reject them one by one; add a new form there deliberately when a step needs
one. Do not fold a check into a preparation step: `terminfo:check`
once ran inside the smoke dependency step, where its failure blocked every smoke
and read as a preparation failure.

Keep the final `Verify Linux x86_64` and `Verify macOS arm64` check names aligned
with the repository ruleset. These gates require every validation job and disable
matrix fail-fast so each reports its own failure instead of cancelling its sibling.

## Workflow policy

Keep workflow scanning scoped to the root `.github` directory. Published vendor
archives can contain upstream workflows that Huterm does not execute. Preserve
those files for archive verification instead of rewriting their action pins.
