# Releasing Printdex

How a release goes from `dev` to a published GitHub release. Nothing is ever
pushed to `main` directly: every change reaches it through a pull request,
so `main` can require one with no bypass.

## Before the first release (once)

The release build signs every file and fails without a key.

1. On your own computer, install `minisign` and run `scripts/release-keys.sh`.
   It asks for a password.
2. Add two repository secrets (Settings › Secrets and variables › Actions):
   - `MINISIGN_KEY`: the whole content of the secret key file it prints
   - `MINISIGN_PASSWORD`: the password you typed
3. Commit `packaging/minisign.pub` (the script writes it) through a PR into
   `dev`. The app has this key built in and refuses updates not signed by it.
4. Keep the secret key file in a password manager, then delete it from disk.
   Losing it means every user has to reinstall once.

## A normal release

1. **Start it.** Actions › **release prepare** › Run workflow, branch `dev`.
   It works out the next version from the commit messages since the last
   tag, bumps `Cargo.toml`, rewrites `CHANGELOG.md`, commits
   `chore(release): vX.Y.Z` to `dev`, and opens a **Release vX.Y.Z** PR into
   `main` with the release notes as its description. It also starts the full
   checks on that PR.
2. **Check and merge.** The PR runs Windows, macOS, MSRV and the bench smoke
   test. Read the notes; when the checks are green, merge with **Create a
   merge commit** (not squash, not rebase).
3. **Publish.** The merge triggers **release tag**, which tags the version
   and starts **release**: installers for all three platforms, signed update
   feeds, and a **draft** GitHub release with the notes. Open Releases, check
   the draft and click Publish. Users' apps see the update only after that.

If more lands on `dev` while the release PR is open, run **release prepare**
again. It refreshes the same PR (version, changelog, notes).

If there is nothing to release (only `docs`, `ci`, `chore`, `refactor`,
`test` or `style` commits since the last tag), release prepare says so and
does nothing.

## Which version comes out

From the [Conventional Commits](https://www.conventionalcommits.org/) since
the last tag, while we're on 0.x:

| Commits since last tag | Next version |
| --- | --- |
| none yet (first release) | `v0.1.0` |
| any `feat` | minor: `0.1.0` → `0.2.0` |
| only `fix`, `perf`, `build` | patch: `0.1.0` → `0.1.1` |

Breaking changes don't jump to 1.0 automatically. Rules live in `cliff.toml`.

## A hotfix

1. Branch `hotfix/<name>` from `main`, commit the fix, push.
2. Actions › **release prepare** › Run workflow, branch `hotfix/<name>`.
3. Merge the PR it opens (merge commit). The tag, build and draft release
   happen as above, and `main` is merged back into `dev` automatically.

## When something goes wrong

- **release tag warns "Run 'release prepare' first"**: something was merged
  into `main` from `dev` without a release commit. Run release prepare on
  `dev` and merge its PR.
- **The release build fails**: fix it through a PR, then rerun **release**
  from Actions with the tag (`vX.Y.Z`) as the ref. The tag already exists,
  so don't run release prepare again for the same version.
- **Back-merge of a hotfix into `dev` fails**: merge `main` into `dev` by
  hand through a PR.

## The pieces

- `.github/workflows/release-prepare.yml` + `scripts/release-prepare.sh`:
  version bump, changelog, release PR
- `.github/workflows/release-tag.yml`: tag on merge, start the build,
  hotfix back-merge
- `.github/workflows/release.yml`: build, sign, draft release
- `cliff.toml`: changelog format and version rules
- `CHANGELOG.md`: written by release prepare, don't edit by hand
