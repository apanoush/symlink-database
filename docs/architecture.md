# Architecture

The crate is a library (`src/lib.rs`) plus a thin binary (`src/main.rs`). Each
module has a single responsibility, and terminal I/O stays in the binary.

```
src/main.rs     CLI entry point: parses args, dispatches to Cli, prints errors
src/lib.rs      module declarations
src/cli.rs      clap command definitions (sync/import/find/chains/list/stats/tui) and orchestration
src/config.rs   loads ~/.config/lndb/config.toml, builds Paths + Skip matcher
src/chain.rs    chain analysis over stored rows (links that point to links)
src/symlink.rs  the Symlink model: validates links, resolves targets, containment checks
src/walker.rs   filesystem traversal, subroot validation, skip-pattern pruning
src/database.rs rusqlite persistence: schema, import/upsert, pruning, queries
src/stats.rs    aggregation over the database into one serializable Stats view model
src/tui.rs      rendering + input for the dashboard (pure draw, no terminal I/O)
src/tui_run.rs  terminal lifecycle + event loop for `lndb tui` (binary-only)
```

`cli.rs` defines the `Tui` subcommand but leaves it unreachable
(`src/cli.rs` lines 104-105, `src/cli.rs:134`); `main.rs` intercepts it and calls
`tui_run` (`src/main.rs` lines 12-15), so the library never opens a terminal.

## Data flow for `sync`

1. `cli::sync` loads `Config` (paths plus skip matcher) from `config.rs`.
2. `walker::Walker` traverses the filesystem, pruning directories that match
   the skip patterns (`filter_entry`), and produces `Symlink` items. The
   traversal itself is sequential (needed for pruning), but entry processing
   (`Symlink` construction, which does the filesystem `canonicalize` calls) is
   parallelized across a `rayon` thread pool (`par_bridge`)
   (`src/walker.rs` lines 72-128).
3. `symlink::Symlink` validates each link: it must be a symlink, its resolved
   target must stay inside `root` (otherwise the link is skipped and an error
   is reported), and it computes the `broken` flag by resolving relative
   targets against the symlink's parent directory. Stored `path` and `target`
   are normalized, with `.` and `..` components removed
   (`src/symlink.rs` lines 43-115).
4. `database::Database` upserts all found symlinks in a transaction, then
   `remove_missing` deletes rows whose path was not found in the scan. The
   deletes are batched `DELETE ... WHERE path IN (...)` queries inside a single
   transaction and scoped to the scanned subtree when `--subroot` is used
   (`src/database.rs` lines 117-198).

## Key invariants

- The walker never descends into a skipped directory
  (`walkdir::filter_entry` skips the entry and its children), so skipped
  subtrees are not scanned at all (`src/walker.rs` lines 81-93).
- `Symlink` stores `path` and `target` relative to `root`; containment and
  broken status are computed against the absolute, resolved paths before
  relativization. Containment uses canonicalized paths, so a `root` that is
  itself a symlink (or a target written with a different spelling) is handled
  correctly (`src/symlink.rs` lines 45-107).
- The walker never records the root entry itself, even when `root` is a
  symlink (`src/walker.rs:103`).
- Scan errors are non-fatal: a single bad symlink is reported through the
  error callback and iteration continues (`src/walker.rs` lines 107-119).
- The database is derived state. It can be dropped and rebuilt from the
  filesystem with a single full `sync` (`src/database.rs` lines 168-198).

## Chain analysis

The `chain` module answers "which symlinks point at another symlink?" from the
database alone. A row participates when its normalized `target` equals another
row's `path`; `collect` follows those edges with a visited set, so cycles
terminate, and reports each starting link, its downstream symlinks and its
terminal path (`src/chain.rs` lines 49-97). The `lndb chains` command renders
the report (`src/cli.rs` lines 242-252).

Because the analysis reads only stored rows, it cannot follow a target that was
never scanned, such as a symlink inside a skipped directory, and an immediate
target outside `root` is stored as its canonical destination instead of the
intermediate symlink. A full `sync` keeps the source rows current.

## Progress reporting

Progress reporting uses `indicatif`. `walker::search_symlinks_with` and
`database::import_many_with` accept callback closures, so the CLI can drive the
spinner and the write bar without the walker or database depending on a UI
crate (`src/walker.rs` lines 72-76, `src/database.rs` lines 117-120).
