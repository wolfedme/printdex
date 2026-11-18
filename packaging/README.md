# Packaging and releases

Everything here is used by `.github/workflows/release.yml`.

- `icons/`: the app icon. `printdex.svg` is the source; the PNG, `.ico`
  (Windows) and `.icns` (macOS) files are rendered from it.
- `minisign.pub`: the public key releases are signed with. The app has it
  built in and only installs updates signed by the matching secret key.
  Until it holds a real key, release builds stop at the signing step and
  the app's updater stays off.

## One-time setup (Max)

1. On your own computer, run `scripts/release-keys.sh`. It makes the key
   pair, writes `packaging/minisign.pub` and prints the secret key.
2. In the repository settings (Secrets and variables › Actions), add
   `MINISIGN_KEY` (the whole secret key file) and `MINISIGN_PASSWORD`.
3. Commit `packaging/minisign.pub`.
4. Store the secret key file in a password manager and delete it from disk.

## Making a release

1. Set the version in the root `Cargo.toml` (`[workspace.package] version`)
   and regenerate the changelog with `git cliff --tag vX.Y.Z -o CHANGELOG.md`.
   Merge that to `main`.
2. Tag `main` with `vX.Y.Z` (or `vX.Y.Z-rc.N` for a test release) and push
   the tag. The workflow checks that the tag matches the version.
3. The workflow builds and packages Linux, Windows and macOS, signs every
   file and creates a **draft** release with notes from the changelog.
   Check it, then publish it. The app's updater only sees published
   releases.

Outputs: Linux AppImage; Windows `Setup.exe` and portable zip; macOS `.dmg`
and portable zip (universal, ad-hoc signed, so the first launch needs
right-click › Open); the Velopack update packages (`.nupkg`) and feeds
(`releases.<channel>.json`); a `.minisig` for every file.

Pull requests that change the workflow or this folder run a dry run: the
same builds and packages, uploaded as workflow artifacts, nothing signed or
released.
