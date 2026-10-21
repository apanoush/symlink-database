# 0004 Chain analysis from stored targets

## Status

Accepted.

## Context

Users need to find symlinks that point to other symlinks, including multi-step
chains and cycles. The database stores `target` as the normalized path the link
names, not the final canonical destination (`src/symlink.rs` lines 64-107), so
a target that is itself a symlink has its own row.

## Decision

Compute chains in a pure library module over the rows returned by
`Database::all` (`src/chain.rs`). A row participates when its `target` equals
another row's `path`. Follow those edges in Rust with a visited set, report the
terminal path, and surface the result through `lndb chains [--json]` only. No
schema change and no TUI tab.

## Alternatives considered

- A `target_is_symlink` column computed during the scan. It would catch targets
  inside skipped directories, but needs a schema migration and still cannot
  describe where those skipped targets lead.
- Following `read_link` on the filesystem at query time. Authoritative for
  skipped and external targets, but slower and reintroduces traversal into an
  analytical command.
- A recursive SQL CTE. Cycle detection and terminal formatting are easier in
  Rust, and `all()` is already the statistics access path.
- Listing every link that participates in a chain instead of only the starting
  links. Rejected as the default because the request is links that point at
  links; downstream symlinks are still shown inside each chain.

## Consequences

- A target symlink that was not scanned, for example one inside a skipped
  directory, cannot be detected. The same applies to an immediate target
  outside `root`, whose stored target is its canonical destination.
- The command loads all rows, which is O(n) memory, consistent with the
  existing statistics path.
- A stale database can report chains that no longer exist until the next
  `sync`.
