# lndb

A small tool that indexes all symlinks found under a root directory into a SQLite database.

The filesystem is authoritative: the database is only for analytics and can be rebuilt from the filesystem at any time. The tool never touches the filesystem (no creation, no deletion, no rewriting of links).

## Install

```sh
cargo build --release
```

The binary is `target/release/lndb`.

## Configuration

Configuration lives at `~/.config/lndb/config.toml` (the XDG config dir for the `net/apanoush/lndb` project).

```toml
[paths]
root = "/../somepath"                                 # root to scan
database = "$VAR/symlink_database/symlinks.db"        # sqlite file to use

[skip]                                             # optional
patterns = [".venv", "target", "node_modules"]     # gitignore-style patterns
```

- `$VAR` and `~` in paths are expanded (see `shellexpand`).
- `[skip]` is optional. Patterns follow gitignore semantics (anchors `/`, `**`, `!` negation) and are matched against paths relative to `root`. Matching directories are pruned during the scan, so they are neither walked nor recorded.

## Usage

```
lndb sync [--subroot <PATH>]
```

Scans `root` (or the `--subroot` subtree, which must be inside `root`), imports every symlink into the database, and deletes database entries that no longer exist on the filesystem. When `--subroot` is given, only entries inside that subtree are candidates for deletion, so unrelated entries elsewhere are left untouched. Symlinks inside skipped directories are removed from the database on the next sync. A progress spinner shows the scan, then a bar shows the database writes.

```
lndb import <PATH>
```

Adds a single symlink (given by path) to the database. The path must be a symlink and must resolve inside `root`.

```
lndb find <TARGET> [--absolute]
```

Lists every symlink in the database whose target matches `TARGET`. The target can be given as an absolute path, an expanded `$VAR`/`~` expression, or relative to the current directory (e.g. `lndb find '$VAR/notes/note.md'`). By default paths are printed relative to `root`; `--absolute` prints absolute paths. Broken symlinks are marked `(broken)`.

```
lndb list [--broken] [--absolute]
```

Pages every symlink path in the database with `less`. `--broken` restricts the list to broken links; `--absolute` prints absolute paths instead of relative ones. In the full list, broken links are marked `(broken)`.

## Known limitations

- A symlink whose target exists but is unreadable (e.g. a path component is permission-restricted) is reported as `broken`, because target existence is checked via `canonicalize`/`stat` and *any* failure counts as broken — the tool does not distinguish `EACCES` from `ENOENT`.

## Storage

The database stores one row per symlink:

| column    | meaning                                              |
| --------- | ---------------------------------------------------- |
| `id`      | auto-incrementing primary key                        |
| `path`    | symlink location, relative to `root`                 |
| `target`  | resolved link target, relative to `root`             |
| `broken`  | `1` when the target does not exist                   |
| `added_at`| unix timestamp of first import                       |

Both `path` and `target` are stored relative to `root`, so entries stay readable and portable. A `sync` transparently migrates old databases containing absolute paths (stale rows are removed, relative rows inserted).

## Architecture

The crate is a library (`src/lib.rs`) plus a thin binary (`src/main.rs`). Each module has a single responsibility:

```
src/main.rs    CLI entry point: parses args, dispatches to Cli, prints errors
src/lib.rs     module declarations
src/cli.rs     clap command definitions (sync/import/find) and orchestration
src/config.rs  loads ~/.config/lndb/config.toml, builds Paths + Skip matcher
src/symlink.rs the Symlink model: validates links, resolves targets, containment checks
src/walker.rs  filesystem traversal, subroot validation, skip-pattern pruning
src/database.rs rusqlite persistence: schema, import/upsert, pruning, queries
```

Data flow for `sync`:

1. `cli::sync` loads `Config` (paths + skip matcher) from `config.rs`.
2. `walker::Walker` traverses the filesystem, pruning directories that match the skip patterns (`filter_entry`), and produces `Symlink` items. The traversal itself is sequential (needed for pruning), but entry processing (`Symlink` construction, which does the filesystem `canonicalize` calls) is parallelized across a `rayon` thread pool (`par_bridge`).
3. `symlink::Symlink` validates each link: it must be a symlink, its resolved target must stay inside `root` (else the link is skipped and an error is reported), and it computes the `broken` flag by resolving relative targets against the symlink's parent directory. Stored `path` and `target` are normalized (`.`/`..` components removed).
4. `database::Database` upserts all found symlinks in a transaction, then `remove_missing` deletes rows whose path was not found in the scan (batched `DELETE ... WHERE path IN (...)` queries inside a single transaction; scoped to the scanned subtree when `--subroot` is used).

Key invariants:

- The walker never descends into a skipped directory (`walkdir::filter_entry` calls `skip_current_dir`), so skipped subtrees are not scanned at all.
- `Symlink` stores `path` and `target` relative to `root`; containment and broken-status are computed against the absolute, resolved paths before relativization. Containment uses canonicalized paths, so a `root` that is itself a symlink (or a link target written in a different spelling) is handled correctly.
- The walker never records the root entry itself, even when `root` is a symlink.
- Scan errors are non-fatal: a single bad symlink is reported on stderr and iteration continues.
- The database is derived state — it can be dropped and rebuilt from the filesystem with a single `sync`.

Progress reporting uses `indicatif`: `walker::search_symlinks_with` and `database::import_many_with` accept callback closures, so the CLI can drive the bars without the walker or database depending on a UI crate.
