# End-to-end test crates

Each directory is a small workspace that reproduces one cause of frequent rebuilds. Its `scenario.toml` lists the steps (cargo commands with their environment, files to write, symlinks, touches), and its README shows the real `cargo frequent` output.

- [Nix dev shell re-seeds `-frandom-seed`](nix-random-seed-drift/README.md)
- [Package selection changes features](package-selection-features/README.md)
- [Activating a Python virtualenv changes `PATH`](virtualenv-path-watch/README.md)
- [mise switches a tool version on `PATH`](mise-tool-switch/README.md)
- [cargo `[env]` changes a watched variable](cargo-config-env/README.md)

`cargo test --test e2e_crates` runs every scenario in a temporary copy and fails when a README no longer matches. `cargo xtask update-e2e` rewrites the outputs.
