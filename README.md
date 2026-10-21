# lndb

A small tool that indexes all symlinks found under a root directory into a SQLite database.

The filesystem is authoritative: the database is only for analytics and can be rebuilt from the filesystem at any time. The tool never touches the filesystem (no creation, no deletion, no rewriting of links).

## Install

```sh
cargo build --release
```

The build produces the `lndb` binary (Cargo package name `symlink_database`) in Cargo's release output directory. Building requires a Rust toolchain with edition 2024 support (Rust 1.85 or newer). `lndb list` also needs `less` on `PATH`.

## Configuration

Configuration lives at `~/.config/lndb/config.toml` (the XDG config dir for the `net/apanoush/lndb` project) and is required by every command:

```toml
[paths]
root = "$HOME/data"                                   # root to scan
database = "$VAR/symlink_database/symlinks.db"        # sqlite file to use

[skip]                                                # optional
patterns = [".venv", "target", "node_modules"]        # gitignore-style patterns
```

`$VAR` expressions are expanded in both paths. `~` is not expanded in config files (it is expanded in `lndb find` targets). See [docs/config.md](docs/config.md) for all keys, defaults and skip-pattern semantics.

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
lndb chains [--json]
```

Lists every symlink whose immediate target is another symlink, together with the rest of the chain. Paths are relative to `root`. A chain that ends at a missing target is marked `(broken)`; a chain that revisits a node is marked `(cycle)`. `--json` emits the report. The analysis reads the database, so a target symlink that was not scanned (for example one inside a skipped directory) is not detected; run `lndb sync` first.

```
lndb list [--broken] [--absolute]
```

Pages every symlink path in the database with `less`. `--broken` restricts the list to broken links; `--absolute` prints absolute paths instead of relative ones. In the full list, broken links are marked `(broken)`.

```
lndb stats [--json] [--limit N] [--depth N]
```

Prints database analytics: totals and broken coverage, files with the most incoming links, folders by symlink count, target dirs by incoming links, the distribution of links per target, and broken links. The folder and target-dir lists group entries by the first `depth` path components of their containing directory (`--depth N`, default 2); an entry directly at the root is grouped under `(root)`. `--limit N` caps the file, folder, target-dir and broken lists (default 20; the links-per-target distribution is shown in full); `--json` emits the full statistics struct as JSON (one view model shared with the TUI).

```
lndb tui
```

Opens an interactive dashboard over the same statistics view model, with seven views: Overview, Files, Folders, Target dirs, Broken, Distribution and Newest. It refuses to start when stdout is not a TTY. Keys: `q`/`Esc` quit, `Tab`/`h`/`l`/left/right switch views, `1`-`7` jump to a view, `j`/`k` scroll, `Home`/`End` or `g`/`G` jump to top/bottom, `[`/`]` change the folder grouping depth, `?` help, `r` reload from the database. On the Files, Folders and Target dirs views, `/` opens a case-insensitive regex filter (`Enter` applies, `Esc` cancels, an empty pattern clears it); the row counts and scrolling follow the filtered set.

## Known limitations

- A symlink whose target exists but is unreadable (e.g. a path component is permission-restricted) is reported as `broken`, because target existence is checked via `canonicalize`/`stat` and *any* failure counts as broken. The tool does not distinguish `EACCES` from `ENOENT`.

## Storage

The database stores one row per symlink:

| column    | meaning                                              |
| --------- | ---------------------------------------------------- |
| `id`      | auto-incrementing primary key                        |
| `path`    | symlink location, relative to `root`                 |
| `target`  | resolved link target, relative to `root`             |
| `broken`  | `1` when the target does not exist                   |
| `added_at`| unix timestamp of first import                       |

Both `path` and `target` are stored relative to `root`, so entries stay readable and portable. A full `sync` rebuilds old databases that contain absolute paths: the relative rows are inserted and the stale absolute rows are pruned. A `--subroot` sync only prunes inside that subtree, so run one full sync after upgrading.

## Architecture

The crate is a library (`src/lib.rs`) plus a thin binary (`src/main.rs`). Each module has a single responsibility, terminal I/O stays in the binary, and the filesystem remains the source of truth. See [docs/architecture.md](docs/architecture.md) for the module map, the `sync` data flow, the key invariants and the progress-reporting design.
