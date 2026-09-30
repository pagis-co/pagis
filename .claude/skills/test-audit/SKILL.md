---
name: test-audit
description: "Use when you write, change, review or prune tests, or when a test is flaky or slow in CI. Authoring gate for new tests, and audit workflow for low-value, duplicate, implementation-coupled, flaky or needlessly heavy tests and the test-only production seams they keep alive."
---

# Test Audit

Three modes use one value bar. Authoring mode gates every new or changed test
when you write it. Audit mode runs focused sweeps for tests that restate
source, duplicate stronger proof, couple to implementation, flake, or cost
more CI time than their risk earns. Campaign mode prunes the whole test
surface of one crate or one area. Before you start a campaign, read
[CAMPAIGN.md](CAMPAIGN.md). Continue a broad audit as separate, coherent pull
requests. Optimize for confidence, not for the number of deleted tests.

Read `CLAUDE.md`, the glossary `CONTEXT.md` and the ADRs in `docs/adr/` that
touch the area before you judge a test. Test names and fixtures use the
glossary terms. Fixtures must not read as real people or real events.

## Where the tests are

- Unit tests: `#[cfg(test)]` modules in `crates/*/src`.
- Integration binaries: `crates/*/tests/main.rs` and the modules it
  includes. The daemon suite is `cargo nextest run -p pagis --test main`.
- The store-trait suite: `pagis-testkit::stores` runs each store test on
  SQLite (`on_sqlite`) and on Postgres (`on_postgres`).
- Docker-real tests: `docker_real::` and `greenmail::` modules, and the
  `egress::` tests. They are `#[ignore]` by default and run with
  `--run-ignored all` when Docker is present. The Computer image must exist
  first (`cargo xtask step computer-image`).
- Browser tests: the `browser_*` modules of the daemon suite drive a real
  Chromium.
- Gate and release tooling: `xtask/tests`.
- TypeScript: `ui/`, `desktop/` and `docs-site/` run Vitest
  (`npm run test` in each).
- `.config/nextest.toml` holds the retries, the slow timeout, the test groups
  that limit Docker and Postgres concurrency, and the `ci` profile filter.

## Authoring gate

Before you add a test, answer four questions. If an answer is missing, do not
add the test yet.

1. Which observable behavior, invariant or independent contract does it
   protect?
2. Which credible regression makes it fail?
3. Why does existing coverage not catch that failure already? Each contract
   has one primary test owner at the strongest boundary. Another layer needs
   its own distinct risk, such as a transport or lifecycle failure that the
   owner cannot reach. Prefer a new row in a table-driven test or a shared
   fixture over a near-duplicate test. Consolidate duplicated setup in the
   same change.
4. Does it need a production seam (a `pub` item, a feature flag, a wrapper, an
   injection hook) that no production caller needs? If yes, move the test to
   the real boundary.

Then check the test against each [junk pattern](#junk-patterns). A match fails
the gate unless the [retention bar](#retention-bar) names the contract that
the test guards independently. A test that breaks under a
behavior-preserving refactor asserts implementation, not behavior. Rewrite it
at the owning boundary before it lands.

A heavy test (Docker, a real browser, Postgres, a real network) must also
name the risk that a cheaper owner cannot reach. Put a new heavy assertion
into an existing heavy test of the same setup when the contract is the same,
instead of starting another container or browser.

A bug regression test must fail on the code before the fix, for the intended
reason, and pass after the repair at the owning boundary. A regression test
that never failed proves the mock, not the fix. One regression at the owning
boundary covers the bug. Do not replay the same scenario at each layer it
crosses.

## Junk patterns

Both modes use this checklist. The authoring gate rejects a new test that
matches one, and an audit hunts for existing tests that match one.

- tests with no assertion that only run code for coverage;
- self-comparisons and identity copies;
- copied fixtures, inventories, manifests or export lists;
- exact source, import or string greps;
- tests of a private predicate or call shape that a real boundary test
  already covers;
- duplicate invocations of the same contract;
- a crate-local replay of a shared helper that its owner already tests;
- tests whose only purpose is to keep a test-only export, global or wrapper
  alive;
- dead production code whose only callers are tests;
- expected values that the helper or renderer under test produces;
- mocks that implement the asserted behavior, or one identical mock for
  different APIs;
- fixtures that supply the receipt, admission or callback order that the
  owner must produce, or persistence asserted against a store that the path
  never writes;
- capability tests that restate a declared flag instead of exercising the
  delivery or acknowledgement that the flag promises;
- negative controls that pass for an unrelated reason, such as a denial from
  a different guard or a rejection that the production path never reaches;
- names or fixtures that promise more than the input exercises;
- assertions on wall-clock time or on a fixed sleep instead of on a value
  (see [Flaky tests](#flaky-tests)).

## Value bar

A test earns its maintenance and CI cost when it protects behavior, a
credible regression, or an independently meaningful contract. In an audit, an
existing test that must change for a behavior-preserving reorganization is
suspect, but not automatically deletable. The authoring gate still rejects new
ones.

Before you judge a candidate, read the complete test and its production
owner: the entry point, callers, callees, sibling implementations,
overlapping tests, CI routing and the relevant history (`git log -S`, the ADR
that introduced the behavior). When the test claims behavior that a
dependency provides, read the dependency source or its types.

## Flaky tests

Nextest retries a failed test once, and the summary marks a pass on the
retry as `FLAKY`. A flaky test is a bug in the test or in the product, never
noise. It also costs CI time: the first attempt usually fails on a deadline,
so the test runs close to twice its time.

- Find the flaky tests in the `Summary` block at the end of the `tests` job
  log. Count them over several runs before you rank them.
- A test that fails only in a full or loaded run almost always reads timing
  instead of behavior. The usual shapes are a fixture that answers by phase
  instead of from the cursor it receives, a read of a value that a different
  write sets, a wait on one transient state, and an assertion on elapsed wall
  time.
- Wait for the value, and name each state that an in-flight object can be
  in. Do not raise a deadline, add a sleep or add retries to hide the cause.
- To reproduce, load every core with busy loops and run the suspect tests
  with high parallelism. Capture the panic payload, not only the line: the
  line is often inside a shared wait helper.
- A flake that reproduces as a product bug is a product fix with its own
  regression test.

## Discovery

Keep discovery read-only, and report the evidence before you edit. For a
broad scope, run parallel lanes:

- unit tests of the crates (`crates/*/src`);
- integration binaries of the crates (`crates/*/tests`), with the daemon
  suite as its own lane;
- the store-trait suite in `pagis-testkit`;
- Docker-real and browser tests;
- `xtask` tests;
- `ui/`, `desktop/` and `docs-site/`;
- a cross-cutting pattern sweep.

Measure before you rank by cost. Each nextest line gives the time of one test
(`PASS [ 19.709s] (3389/3424) pagis-vault::main docker_real::...`). Sum the
times per binary and per module from the log of a green `tests` job on
`main`, and compare the sum with the wall time: the last tests of a run
often decide the wall time on their own. Outside campaign mode, prefer a few
high-confidence candidates over a large speculative inventory.

## Retention bar

Keep a test when it independently enforces a public API, plugin, protocol,
configuration, migration, storage, security, platform, default, prompt-text,
generated-contract (OpenAPI, the UI API types), package, release or
architecture contract. Also keep:

- call order when the order is observable behavior;
- regressions with a credible failure mode;
- source inspection when it is the cheapest independent guard: it fails when
  the contract changes (the user-facing key, byte or path) and survives a
  rename of an identifier;
- a retained test that fails on the baseline. Treat it as a possible product
  bug, reproduce it and repair the owner. Do not delete it.

Slow is not a reason to delete. Slow is a reason to find the cheaper owner of
the same contract, to consolidate heavy tests that share a setup, or to fix
the harness that makes the test slow. A test that resembles implementation can
still be the independent contract. Prove otherwise before you remove it.

## Candidate evidence

Record each field below before you edit. If a field is missing, the candidate
is not ready for deletion:

- the exact test name and location;
- the failure it can actually detect;
- the non-test callers of the production code or support seam it covers;
- the stronger proof that remains at the owning boundary, or why no proof is
  necessary;
- the relevant history and the reason why the test or seam exists;
- the production or test-support code that the deletion unlocks;
- the CI seconds it costs, from a measured run;
- the risk, and the focused validation command.

## Edit shape

Choose one coherent batch at one owning boundary. Delete obsolete test-only
exports, wrappers and dead production paths. Do not keep aliases. Move
retained regressions to their canonical owners. Consolidate repeated package
or dependency assertions into one generic contract.

Prefer a net-negative line count in production code. Do not add replacement
tests that restate the same implementation. Do not turn uncertain candidates
into cleanup to increase the deletion count.

## Validation

1. Run the smallest owner and sibling tests:
   `cargo nextest run -p <crate> -E 'test(/<name>/)'`, with
   `--run-ignored all` for Docker-real tests. Where nextest is not
   installed, use `cargo test -p <crate>` (all targets, not only `--lib`).
2. For a removed source grep, run the command or check that owns the real
   contract (for example `cargo xtask pins --check`).
3. For a flake fix, show the failure under load before the fix and a clean
   series of runs under the same load after it.
4. Run `cargo fmt --all` and `git diff --check`.
5. Run `cargo xtask dev`. It selects the changed crates, their reverse
   dependants and the changed UI, desktop or documentation checks.
6. Read `git diff --numstat`. Report production and tooling lines apart from
   test and test-support lines.

## Landing and continuation

Commit, push or open a pull request only when you are authorized. Land one
coherent pull request at a time, and wait for the CI workflow to pass. After
it lands, refresh from `main` and run read-only discovery again for the next
high-confidence batch.

## Handoff

Report:

- the root cause and the low-value categories you removed;
- the flaky tests you fixed, with their root cause;
- the simplifications of production owners;
- the false positives you kept, and why they stay valuable;
- the focused and full proof that you ran;
- production lines against test lines;
- the CI seconds saved, measured on a run, not estimated;
- the pull request and its merge state;
- the named follow-ups.

Adapted from the test-audit skill of OpenClaw (MIT License, Copyright (c)
2026 OpenClaw Foundation).
