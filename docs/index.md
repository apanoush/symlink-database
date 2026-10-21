# lndb documentation

`lndb` indexes every symlink under a root directory into a SQLite database for
analytics. The filesystem is authoritative: the database is derived state and
can be rebuilt with `lndb sync`. The tool never creates, deletes or rewrites
links.

- [README](../README.md): install, command usage, storage schema and known
  limitations.
- [Configuration](config.md): every `config.toml` key, path expansion rules and
  skip-pattern semantics.
- [Architecture](architecture.md): module map, `sync` data flow, key invariants
  and progress reporting.

## Decisions

- [0001 Derived SQLite database](decisions/0001-derived-sqlite-database.md)
- [0002 Gitignore-style skip patterns](decisions/0002-gitignore-style-skip-patterns.md)
- [0003 Parallel symlink processing](decisions/0003-parallel-symlink-processing.md)
- [0004 Chain analysis from stored targets](decisions/0004-chain-analysis-from-stored-targets.md)
