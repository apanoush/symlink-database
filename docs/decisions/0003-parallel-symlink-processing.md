# 0003 Parallel symlink processing

## Status

Accepted.

## Context

`sync` spends most of its time resolving link targets, which calls
`fs::canonicalize` per symlink. The directory walk itself is cheap by
comparison, and it must stay sequential because skip patterns prune whole
subtrees through `walkdir::filter_entry`.

## Decision

Keep the `walkdir` traversal sequential and parallelize per-entry processing
with `rayon` (`par_bridge`) on the resulting iterator
(`src/walker.rs` lines 72-128). Symlink construction, resolution and broken-status
checks run on the rayon pool. Progress and error callbacks are shared behind a
`Mutex` so the API stays `FnMut`.

## Alternatives considered

- Fully sequential processing. Simplest and deterministic, but canonicalize
  calls dominate runtime on large trees.
- A parallel directory walker. It would complicate pruning and subroot
  validation, and the walk is not the bottleneck.
- Async I/O with a runtime. Extra dependency and complexity for a CLI that
  otherwise blocks, with no clear win for filesystem metadata calls.

## Consequences

- The order of discovered symlinks is nondeterministic. Results are keyed by
  the unique `path` column, so database state does not depend on order.
- Callbacks are serialized by a mutex, which is acceptable because they only
  update progress bars.
- Errors remain non-fatal: a failing entry is reported and skipped
  (`src/walker.rs` lines 107-119).
