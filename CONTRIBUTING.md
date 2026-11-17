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
