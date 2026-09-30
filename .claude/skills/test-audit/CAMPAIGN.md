# Test-pruning campaign

Campaign mode prunes the whole test surface of one crate or one area in one
pull request: for example `pagis-vault`, the store-trait suite, or the
browser tests of the daemon. The value bar, the retention bar, the candidate
evidence and the validation in [SKILL.md](SKILL.md) apply to each lane. This
file gives the order of work. Each step ends on its completion criterion. Do
not start the next step early.

## 1. Baseline

At a pinned `main` commit, record the test and test-support line counts of
the area, the pass or fail state of each test, and the CI seconds of each
test from the log of a green `tests` job. Keep baseline failures and flaky
tests in their own list: they are possible product bugs, not stale tests.

Done when each test in scope has a recorded baseline result and time.

## 2. Lanes and inventory

Split the surface into **lanes** along production owner boundaries, not along
file names. Include the cases of the area at shared boundaries: the daemon
suite in `crates/pagis/tests`, the store-trait suite, the Docker-real tests
and the UI tests that exercise it.

Done when each test and each shared case of the area belongs to exactly one
lane.

## 3. Read-only ledger per lane

Give each lane to its own read-only agent. The agent reads each assigned test
in full, with its table rows and fixtures. It also reads the production
owners with their entry points, callers, history and CI routing. Each test
goes into a written **ledger** with one mark. A table-driven test is one
entry unless its rows need different marks; then mark each row.

- `R`: retain. Name the contract and the bug it catches. A retained test that
  only moves to a better file stays `R`, with the move noted.
- `F`: retain the contract but repair the test: a vacuous assertion, a wait
  on timing instead of a value, a heavy setup that a shared one can replace.
- `C`: consolidate. Name the owner that absorbs the assertion first: a table
  row of a sibling, a stronger boundary suite, or the shared owner in another
  crate.
- `D`: delete. Name the proof that remains, or why no contract exists.

Judge a test by its assertions, not by its name.

Done when each test in the lane has a mark and a line of evidence.

## 4. Layer plan per lane

The ledger is input, not the edit list. A second read-only pass starts from
the ledger and looks for the redundant **layer**: for example, several
integration tests that replay one shared helper through the same mock,
beside a stronger test at the real boundary. Name the **keeper** suite for
each contract. Prefer the real boundary with a fake network or a fake
provider over a mocked collaborator. Correct any ledger errors that this
pass finds.

Done when each lane plan names its retired files, the keeper of each
contract, the assertions to carry into the keepers, and the test-only
production seams it unlocks.

## 5. Cutover

Edit lane by lane. Let one owner make the changes to shared harnesses such as
`pagis-testkit`. With each lane, remove the test-only production seams it
unlocks: injection parameters, getters, reset functions and indirection
layers. Update `.config/nextest.toml` when a test group or filter names a
moved or removed test. Put durable test rules for the area in `CLAUDE.md` or
in a comment of the keeper suite, drawn from mistakes that the campaign
found.

Done when each lane plan is applied and the keepers of each lane pass.

## 6. Preservation review

Before you claim completion, let independent reviewers compare the deleted
coverage against the keepers, one reviewer for each boundary group. They look
for contracts that lost their only proof. They also look for new assertions
that cannot fail, such as a rejection that the production code never reaches.

For each restored contract, make one deliberate **mutation** of the production
owner and confirm that the keeper goes red. Then restore the source byte for
byte.

Done when each reported gap is restored or rejected with source evidence, and
each restored contract has a caught mutation.

## 7. Product defects

A baseline failure or flake that survives into a keeper is a bug report. Fix
it at its owner in a separate commit, and prove it through the real user
flow, with a **control** run that reverts the fix and shows the old behavior.
Record unrelated product discrepancies as follow-ups. Do not fix them in the
campaign.

Done when each repaired defect has a failing control and a passing candidate
on the same harness.

## 8. Reconcile and hand off

A campaign outlives many `main` commits. Merge `main` into the campaign
branch; do not rebase a long branch. When `main` changed a file that the
campaign deleted, keep the deletion, port the new contract into the keeper,
and confirm that each new regression test from `main` still has a home. Run
the whole suite of the area again on the merged head.

Hand off with the report of [SKILL.md](SKILL.md), plus:

- the test and test-support line counts before and after, with production
  counted apart;
- the lanes, the retired layers and the keepers;
- the preservation gaps found and their mutations;
- the product defects with their control and candidate proof;
- the CI seconds of the area before and after, from measured runs.
