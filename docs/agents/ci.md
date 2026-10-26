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
anonymous API limit while verifying action pins. Pass it to zizmor as
`ZIZMOR_GITHUB_TOKEN` as well; see [workflow policy](#workflow-policy).

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

CI installs Zig through `.github/actions/setup-zig`, which runs
`mise install --locked zig` in its own step. Keep `zig` out of CI's Mise action
install lists so that step keeps its time bound and retry. The action gives the
install its own `MISE_CACHE_DIR` under `RUNNER_TEMP` and caches that directory,
which holds Mise's download store, keyed by runner, Zig version, and Mise
version. Mise reuses a stored archive only when it matches the `mise.lock`
checksum, and it checks the minisign signature on every install. A cache hit
therefore still fetches the `.minisig`. Mise tries the serving mirror,
ziglang.org, and then every other mirror for it. Releases before 2026.10.7
asked only the serving mirror, so one rate-limited mirror (HTTP 429) failed the
job. A mirror that sends nothing costs about two minutes (a 30-second read
timeout and three retries), but one that sends slowly is limited only by the
30-minute download timeout. Jobs therefore bound the step to 4 minutes and
retry once within 6 minutes, which reshuffles the mirrors.

Every install, cached or not, also fetches `ziglang.org/download/index.json`
to find the tarball URL. When that fails, Mise 2026.10.17 falls back to the
filename order Zig used before 0.14 (`zig-macos-aarch64-…`), so a ziglang.org
outage fails the install.

The Mise action saves its install cache right after its own install step, so
tools installed later in a job, such as Zig and the `cargo:` tools, never reach
it. The setup-zig action also points `ZIG_GLOBAL_CACHE_DIR` and
`ZIG_LOCAL_CACHE_DIR` under `RUNNER_TEMP`, where restore-ghostty and
save-ghostty expect Zig's package cache. Release jobs install Zig through the
Mise action and use no Actions cache. Reference local actions as
`./.github/actions/...` with a `zizmor: ignore[self-repository]` comment:
actionlint rejects GitHub's `$/` self-repository syntax.

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

`ci:workflows` runs zizmor's online audits whenever a token is set, and fails
in CI without one. Dependabot alerts skip SHA-pinned actions, so zizmor's
`known-vulnerable-actions` audit is the only advisory check these pins get.
A newly published advisory can therefore fail the Policy job on an unrelated
pull request. Bump the affected pin, or add a scoped ignore to `zizmor.yml`
until a fixed release clears the cooldown. The pre-commit hook stays
`--offline` for speed.

actionlint lints only workflow files, using whichever `shellcheck` is on
`PATH`: the runner image's copy in CI, and none at all on a machine without
it. The `run:` scripts in composite actions under `.github/actions` get
zizmor's security audits but no shell linting.

Runners force Node 20 actions onto Node 24 and annotate each run with a
deprecation warning. Check `runs.using` in an action's `action.yml` at its
pinned SHA rather than relying on annotations, which cover only workflows that
ran.
