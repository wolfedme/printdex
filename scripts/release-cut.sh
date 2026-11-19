#!/usr/bin/env bash
# Cuts a release from the checked-out commit (release-cut.yml runs it on
# every push to main): works out the next version from the conventional
# commits since the last tag with git-cliff, bumps the workspace version,
# rewrites CHANGELOG.md, commits "chore(release): vX.Y.Z" and tags it.
# Pushing is left to the caller. Needs git-cliff.
#
# Writes `tag=vX.Y.Z` to $GITHUB_OUTPUT (empty when there is nothing to
# release). Safe to run twice: a tagged HEAD or no releasable commits is a
# no-op.
set -euo pipefail

out="${GITHUB_OUTPUT:-/dev/null}"
say() { echo "$*"; if [[ -n "${GITHUB_STEP_SUMMARY:-}" ]]; then echo "$*" >> "$GITHUB_STEP_SUMMARY"; fi; }

if tag=$(git describe --tags --exact-match --match 'v[0-9]*' HEAD 2>/dev/null); then
  say "HEAD is already released as $tag."
  echo "tag=" >> "$out"
  exit 0
fi

last=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
next=$(git cliff --bumped-version 2>/dev/null)
if [[ -n "$last" && "$next" == "$last" ]]; then
  say "No feat/fix/perf/build commits since $last: nothing to release."
  echo "tag=" >> "$out"
  exit 0
fi
version="${next#v}"

# Workspace version, and the workspace crates' entries in Cargo.lock.
sed -i "/^\[workspace.package\]/,/^\[/ s/^version = \".*\"/version = \"$version\"/" Cargo.toml
awk -v v="$version" '
  /^name = "printdex(-[a-z]+)?"$/ { print; getline; if ($0 ~ /^version = /) $0 = "version = \"" v "\""; }
  { print }
' Cargo.lock > Cargo.lock.new && mv Cargo.lock.new Cargo.lock
grep -q "^version = \"$version\"" Cargo.toml || { echo "::error::could not bump Cargo.toml"; exit 1; }

git cliff --tag "$next" -o CHANGELOG.md

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "chore(release): $next"
git tag -a "$next" -m "Printdex $version"
say "Cut $next (previous: ${last:-none})."
echo "tag=$next" >> "$out"
