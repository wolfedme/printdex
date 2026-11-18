# Contributing to Printdex

Thanks for your interest in Printdex!

**Issues, bug reports and ideas are very welcome.** Open an issue on GitHub
and describe what you saw, what you expected, and (for bugs) your operating
system and how your model library is stored (local disk, USB drive, NAS).

**Pull requests from outside contributors are not being accepted yet.** This
keeps the licensing of the project flexible while it is young. This may
change later; when it does, this file will say how to contribute code.

## Maintainer notes

### Database

Queries are checked at compile time by sqlx against metadata committed in
`.sqlx/`, so a normal build needs no database (`SQLX_OFFLINE=true` in CI).
After adding or changing a query or a migration, regenerate the metadata:

```sh
cargo install sqlx-cli --no-default-features --features sqlite --locked
export DATABASE_URL=sqlite://$PWD/target/dev.db
sqlx database create
sqlx migrate run --source crates/printdex-core/migrations
cargo sqlx prepare --workspace
```

### Tests

printdex-core's integration tests build as one binary: add a new file under
`crates/printdex-core/tests/it/` and a `mod` line in `tests/it/main.rs`
rather than a new top-level file in `tests/`. Each top-level file links the
whole engine (rustic, OpenDAL, sqlx) again, which costs CI time and disk.

If a backup test is slow, check whether the crate doing the work is in the
`[profile.dev.package]` list in `Cargo.toml`: unoptimised crypto and
compression run several times slower.
