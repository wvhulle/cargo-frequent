# Showcase

Each directory is a small workspace that reproduces one cause of frequent rebuilds, with the real `cargo frequent` output in its README.

- [Nix re-seeds `-frandom-seed`](nix-random-seed-drift/README.md)
- [Package selection changes features](package-selection-features/README.md)
- [A build script watches `PATH`](volatile-path-watch/README.md)

Run `cargo xtask sync-showcase` after changing the output format, and `cargo xtask sync-showcase --check` to verify the READMEs are current.
