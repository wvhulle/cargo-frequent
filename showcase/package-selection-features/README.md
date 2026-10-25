# Package selection changes features

`alpha` enables the `extra` feature of `shared`, `beta` does not. Cargo unifies features over the packages a command selects, so `cargo check -p beta` and `cargo check -p alpha` compile two different `shared` units. Alternating between commands (`-p`, `default-members`, `--workspace`, `cargo xtask`) keeps recompiling everything above `shared`.

```sh
cargo check -p beta
cargo check -p alpha
cargo frequent
```

<!-- showcase:package-selection-features -->
```text
1 cause recompiles 3 of 3 workspace members (last 24h)

1  feature sets depend on which packages a command selects   (1 crate got new unit hashes in the last 24h)
   ├─ change      shared +extra
   ├─ recompiles  all 3 workspace members
   └─ fix         feature unification depends on which packages a command selects
                  unify per workspace: `[resolver] feature-unification = "workspace"` in .cargo/config.toml (nightly also needs `[unstable] feature-unification = true`), or enable the feature in every dependent.
```
<!-- /showcase -->

The output above comes from running these commands on this workspace; `cargo xtask sync-showcase` regenerates it.
