# Contributing to Printdex

Thanks for your interest in Printdex!

**Issues, bug reports and ideas are very welcome.** Open an issue on GitHub
and describe what you saw, what you expected, and (for bugs) your operating
system and how your model library is stored (local disk, USB drive, NAS).

**Pull requests from outside contributors are not being accepted yet** (they are closed automatically). This
keeps the licensing of the project flexible while it is young. This may
change later; when it does, this file will say how to contribute code.

Security problems: please report them privately, see [SECURITY.md](SECURITY.md).

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

### Branches and releases

- `dev` is the default branch. Feature and fix PRs go into `dev`; CI runs
  the Linux job (fmt, clippy, tests) on the PR and again on the push to
  `dev`, which also saves the build cache every PR starts from.
- A release is a PR from `dev` into `main`. That PR runs the heavy jobs
  once: Windows, macOS, MSRV and the bench smoke. Merge it with "Create a
  merge commit", not squash.
- Merging `dev` (or a `hotfix/*` branch) into `main` cuts the release (`.github/workflows/release-cut.yml`
  and `scripts/release-cut.sh`): git-cliff picks the next version from the
  conventional commits since the last tag (`feat` bumps the minor while
  we're on 0.x, `fix`/`perf`/`build` the patch), bumps `Cargo.toml`,
  rewrites `CHANGELOG.md`, tags `vX.Y.Z`, starts `release.yml` (installers,
  signed update feeds, a draft GitHub release with the notes) and merges
  `main` back into `dev`. Publishing the draft is a manual step.
- Hotfix: branch `hotfix/<name>` from `main`, PR into `main` (full CI), merge; the same
  workflow releases it and brings `dev` up to date.
- Commit messages follow Conventional Commits (`feat(app): …`,
  `fix(core): …`); `docs`, `ci`, `chore`, `refactor`, `test` and `style`
  commits stay out of the changelog.
