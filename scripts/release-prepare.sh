#!/usr/bin/env bash
# Prepares a release on the checked-out branch (release-prepare.yml runs it
# on dev or a hotfix/* branch): works out the next version from the
# conventional commits since the last tag with git-cliff, bumps the
# workspace version, rewrites CHANGELOG.md and commits
# "chore(release): vX.Y.Z". Tagging happens after the release PR merges into
# main (release-tag.yml). Pushing is left to the caller. Needs git-cliff.
#
# Writes `version=X.Y.Z` to $GITHUB_OUTPUT (empty when there is nothing to
# release). Safe to run twice: an already-prepared version or no releasable
# commits is a no-op.
set -euo pipefail

out="${GITHUB_OUTPUT:-/dev/null}"
say() { echo "$*"; if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then echo "$*" >> "$GITHUB_STEP_SUMMARY"; fi; }

current=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)
last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
next=$(git cliff --bumped-version 2>/dev/null)
version="${next#v}"

if [[ -n "$last" && "$next" == "$last" ]]; then
  say "No feat/fix/perf/build commits since $last: nothing to release."
  echo "version=" >> "$out"
  exit 0
fi
if [[ "$(git log -1 --format=%s)" == "chore(release): $next" ]]; then
  say "$next is already prepared on this branch."
  echo "version=$version" >> "$out"
  exit 0
fi

# Workspace version, and the workspace crates' entries in Cargo.lock.
if [[ "$current" != "$version" ]]; then
  sed -i "/^\[workspace.package\]/,/^\[/ s/^version = \".*\"/version = \"$version\"/" Cargo.toml
  awk -v v="$version" '
    /^name = "printdex(-[a-z]+)?"$/ { print; getline; if ($0 ~ /^version = /) $0 = "version = \"" v "\""; }
    { print }
  ' Cargo.lock > Cargo.lock.new && mv Cargo.lock.new Cargo.lock
  grep -q "^version = \"$version\"" Cargo.toml || { echo "::error::could not bump Cargo.toml"; exit 1; }
fi

git cliff --tag "$next" -o CHANGELOG.md

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "chore(release): $next"
say "Prepared $next (previous: ${last:-none})."
echo "version=$version" >> "$out"
