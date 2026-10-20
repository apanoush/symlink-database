# 0001 Derived SQLite database

## Status

Accepted.

## Context

Symlinks are already stored on the filesystem. The tool needs analytics:
counts, broken links, the most linked targets, folder distributions and import
history. Reading the whole tree for every query is slow, and a durable,
queryable snapshot is useful across runs.

## Decision

Store one row per symlink in SQLite via `rusqlite` with the bundled SQLite
build, and treat the database as derived state:

- the filesystem is authoritative, so the database can be deleted and rebuilt
  with one full `sync` (`src/walker.rs`, `src/database.rs`);
- `path` is `TEXT UNIQUE NOT NULL`, so upserts are keyed by link location
  (`src/database.rs` lines 67-74);
- `path` and `target` are stored relative to `root`, which keeps rows portable
  and readable (`src/symlink.rs` lines 92-107);
- `added_at` records the first import and is not rewritten by later upserts
  (`src/database.rs` lines 97-103, `src/database.rs` lines 131-137);
- `broken` is refreshed on every import (`src/database.rs` lines 100-102,
  `src/database.rs` lines 134-137).

## Alternatives considered

- Re-scan the filesystem for every query. Simpler and always current, but too
  slow for repeated analytics over large trees.
- A JSON or custom flat file. No query language, no efficient grouping, and
  reimplementing pruning and aggregation.
- Storing absolute paths. Not portable if `root` changes, and it makes
  containment checks harder to audit. A full `sync` converts old absolute-path
  rows to relative ones by pruning them as stale
  (`src/database.rs` lines 168-198).

## Consequences

- The database can drift from the filesystem between syncs; that is accepted.
- SQLite file locking governs concurrent writers. The current CLI is
  single-process, so no extra coordination is implemented.
- The bundled SQLite build avoids depending on a system library.
