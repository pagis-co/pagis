- Study how established products solve the problem before designing a solution. Adopt their proven patterns and conventions rather than inventing the approach from scratch.
- Do not preserve backward compatibility. Remove obsolete paths instead of adding compatibility layers, fallbacks or migrations.
- Choose the simplest implementation that fully meets the current requirements. Avoid speculative abstractions, configuration and indirection.
- Grow the system in layers. Start from the smallest version that works end to end, and add each capability on top of a product that already works. Never trade a working product for unfinished complexity.
- Keep components modular and concerns clearly separated.
- Prefer established, well-maintained libraries when they reduce overall complexity or improve reliability. Do not reimplement common functionality without a clear reason.
- Lean on the dependencies already in the project before writing your own implementation or adding packages. Do not assume a library lacks or contains a capability without checking its documentation and types.
- Make broader architectural decisions for the long-term. Do not accept a stopgap that only works for now and is meant to be replaced later.
- Follow S.O.L.I.D principles when writing code.
- Use Test Driven Development (TDD). Tests are stored and not throwaway.
- Follow ASD-STE100 Simplified Technical English standard when writing documents, comments etc., as well as when communicating with the user.
- Documents state the product as it is, in the present tense. They contain no issue or pull request numbers, no dates, no stage markers ("Stage 1", "for now", "later") and no release or handoff notes. A part that is not built is named as not built.

## Domain documentation

`CONTEXT.md` at the repository root is the glossary: it holds the
canonical term for every domain concept. Use those terms in code, in
issue titles and in prose, and do not drift to a synonym the glossary
avoids. A concept that is not in the glossary is a signal: either the
language is new and needs a second look, or the glossary has a gap.

`docs/adr/` holds the standing architecture decisions. Read the records
that touch an area before you change it. When a change contradicts a
record, say so and reopen the decision instead of working around it.

## Worktrees

A session that builds in a git worktree points Cargo at the main checkout's `target/` before its first cargo command:

```bash
export CARGO_TARGET_DIR="$(git worktree list | head -1 | awk '{print $1}')/target"
```

The shared directory reuses the main checkout's compiled third-party crates, so a worktree build finishes in minutes. Cargo holds a lock on the directory, so parallel worktree builds queue and never corrupt it. A worktree without this variable compiles the whole workspace from scratch into its own `target/` (about 40 GB and over ten minutes for clippy alone), and parallel worktrees then compete for the CPU.

Use `cargo xtask dev` for the edit loop. It checks changed Rust packages and their reverse dependants, and it selects the changed UI, desktop, or Computer checks. Shared build configuration and unknown paths select the full gate.

Use `cargo xtask full` for every gate step, and `cargo xtask step <name>...` for the named ones. A pull request merges when the CI workflow passes; its jobs run the same steps.

Cargo keys the workspace crates on the worktree path, so each worktree writes its own copy of them. A binary built in one worktree can still be reused in another, so a path into the repository is read at run time (`std::env::var_os("CARGO_MANIFEST_DIR")`), never baked in with `env!`, which would hold the path of a worktree that no longer exists. Run focused tests with `cargo nextest run`, not `cargo test`: nextest gives per-test timing and retries.
