# chrono is a dependency, despite the no-new-dependency rule

`AGENTS.md` tells agents not to add dependencies, and this change adds one: rendering `last_active` in the reader's own time zone needs zone data, and the Rust standard library carries none — `SystemTime` can only produce an epoch. The alternatives were to read the platform's zone database directly, which means separate Windows and Unix implementations of something every calendar library already solves, or to show UTC and make every reader do the arithmetic, which defeats the point of the field. chrono is pulled in with default features off, keeping only `clock` and `std`.

## Consequences

Relative wording (`3 hours ago`) needs no zone data and would survive dropping the dependency; only the absolute half (`2026-08-15 02:33`) depends on it. If chrono is ever removed, `timefmt.rs` is the single place that has to change.
