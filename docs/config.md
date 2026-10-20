# Configuration

`lndb` reads one TOML file at startup for every command. On Linux it lives at
`~/.config/lndb/config.toml`, derived from
`directories::ProjectDirs::from("net", "apanoush", "lndb")`
(`src/config.rs` lines 65-68); the `directories` crate also honors `XDG_CONFIG_HOME`.
The file is required: a missing file aborts with `ConfigFileNotFound`
(`src/config.rs` lines 70-74).

## Keys

| Key | Required | Type | Default | Meaning |
| --- | --- | --- | --- | --- |
| `paths.root` | yes | string | none | Root directory to scan. Must exist and be a directory for `sync` (`src/walker.rs` lines 34-36). |
| `paths.database` | yes | string | none | SQLite file used for analytics. Created by SQLite when absent (`src/database.rs` lines 47-60). |
| `skip.patterns` | no | array of strings | `[]` | Gitignore-style patterns excluded from the scan (`src/config.rs` lines 101-113). |

Unknown keys are ignored by the deserializer.

## Path expansion

Both path values pass through `shellexpand::env`, so `$VAR` and `${VAR}` are
expanded (`src/config.rs` lines 90-91). Tilde is not expanded in config files:
`~` is only expanded for `lndb find` targets (`src/cli.rs` lines 222-224). An
undefined variable is an error (`ConfigError::EnvVariableError`). Relative
paths are used as written, so prefer absolute paths in the config file.

## Skip patterns

Patterns are compiled into an `ignore` crate gitignore matcher rooted at
`paths.root` (`src/config.rs` lines 106-113) and matched against paths relative to
that root (`src/walker.rs` lines 85-90). The syntax is gitignore syntax:

- a basename pattern such as `.venv` matches at any depth;
- a leading `/` anchors the pattern at the root;
- `**` matches across directories;
- `!` negates an earlier pattern.

The predicate applies to every walked entry. Matching directories are pruned,
so nothing below them is scanned or recorded; matching files and symlinks are
skipped as well (`src/walker.rs` lines 81-93). Because skipped entries are not
recorded, symlinks that a later sync no longer finds are deleted from the
database by that sync (`src/database.rs` lines 168-198).

An absent `[skip]` section or an empty `patterns` list disables filtering
(`src/config.rs` lines 101-104).

The gitignore semantics are covered by unit tests in `src/config.rs` lines 134-174.

## Example

```toml
[paths]
root = "$HOME/data"
database = "$HOME/.local/share/lndb/symlinks.db"

[skip]
patterns = [".venv", "target", "node_modules", "*.iso"]
```

Run `lndb sync` after editing the file; every command re-reads it on startup.
