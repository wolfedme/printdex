#!/usr/bin/env bash
# Makes the minisign key pair that signs Printdex releases.
#
# Run once, on your own computer (needs `minisign`: apt install minisign,
# brew install minisign, or scoop install minisign). It asks for a password.
#
#   scripts/release-keys.sh
#
# Then:
#   1. Add two repository secrets (Settings > Secrets and variables > Actions):
#      MINISIGN_KEY       the whole content of the secret key file it prints
#      MINISIGN_PASSWORD  the password you typed
#   2. Commit packaging/minisign.pub (written by this script). The app has
#      this public key built in and refuses updates not signed by the key.
#   3. Keep the secret key file somewhere safe (a password manager), then
#      delete it from disk. Losing it means users must reinstall once.
set -euo pipefail

command -v minisign >/dev/null || { echo "minisign is not installed" >&2; exit 1; }
root=$(cd "$(dirname "$0")/.." && pwd)
key="${PRINTDEX_KEY_FILE:-$HOME/printdex-release.key}"
if [ -e "$key" ]; then
  echo "$key already exists; move it away first" >&2
  exit 1
fi
minisign -G -p "$root/packaging/minisign.pub" -s "$key" -c "Printdex release key"
echo
echo "Public key written to packaging/minisign.pub (commit it)."
echo "Secret key: $key"
echo "Paste its whole content into the MINISIGN_KEY secret:"
echo
cat "$key"
