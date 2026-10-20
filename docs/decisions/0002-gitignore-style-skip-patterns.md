# 0002 Gitignore-style skip patterns

## Status

Accepted.

## Context

A root tree often contains dependency and build directories (`.venv`,
`target`, `node_modules`) with many links that are irrelevant to analytics.
Users need to exclude them without maintaining a separate scan script, and the
exclusion rules should be familiar.

## Decision

Compile the optional `skip.patterns` list into an `ignore` crate gitignore
matcher rooted at `paths.root` (`src/config.rs` lines 100-126). Match patterns
against paths relative to `root` and prune matching entries during the walk
(`src/walker.rs` lines 81-93).

Gitignore semantics were chosen because they cover the common cases without
new syntax: basename patterns match at any depth, a leading `/` anchors at the
root, `**` crosses directories, and `!` re-includes earlier matches.

## Alternatives considered

- Hardcoded exclusions. No configuration, but users cannot adapt the list.
- A regex or glob list. More expressive in some cases, but no negation
  semantics and a steeper learning curve for a list of directory names.
- Reading nested `.gitignore` files. Convenient, but it would couple scans to
  version-control layout and to files outside the config.
- A separate ignore file with its own format. Extra file and parser for little
  benefit.

## Consequences

- Patterns apply to files as well as directories; matching directories are
  pruned so their subtrees are never walked (`src/walker.rs` lines 81-93).
- Skipped entries are absent from the scan results, so the next sync removes
  their rows from the database (`src/database.rs` lines 168-198).
- Only the configured patterns are applied; nested ignore files are not read.
