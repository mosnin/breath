# Sidecar binaries

`tauri.macos.conf.json` bundles the RuView sensing server into
`Piranha.app/Contents/MacOS/sensing-server`. Tauri expects the file here
named with the Rust target triple, e.g.

    binaries/sensing-server-aarch64-apple-darwin
    binaries/sensing-server-x86_64-apple-darwin
    binaries/sensing-server-universal-apple-darwin

`scripts/build-macos.sh` builds and places it automatically. Built binaries
are git-ignored.
