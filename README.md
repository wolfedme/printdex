# printdex
Your STL library, organized. Local-first, blazing fast, built in Rust.

## Status

Early development. The desktop app shows your libraries as a searchable grid; previews, the model view, tags and the print log come in later milestones (see the issues).

## Run it

Needs Rust (stable, 1.94 or newer).

```sh
# Desktop app
cargo run --release -p printdex-app

# Use a separate data folder, or force light/dark
cargo run --release -p printdex-app -- --data-dir /tmp/printdex-test --mode dark

# Command line: scan a folder, search, stats
cargo run --release -p printdex-cli -- scan ~/Models
cargo run --release -p printdex-cli -- search dragon
```

In the app: **Add library** picks a folder and scans it. `/` or Ctrl+F jumps to search, arrow keys move through the grid, Esc clears the search.

Printdex never changes, moves or uploads your model files. Its database lives in your OS data folder (`~/.local/share/printdex` on Linux).
