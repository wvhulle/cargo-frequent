# Package selection changes features

`alpha` enables the `extra` feature of `shared`, `beta` does not. Cargo unifies features over the packages a command selects, so `cargo check -p beta` and `cargo check -p alpha` compile two different `shared` units. Alternating between commands (`-p`, `default-members`, `--workspace`, `cargo xtask`) keeps recompiling everything above `shared`.

```sh
cargo check -p beta
cargo check -p alpha
cargo frequent
```

<!-- showcase:package-selection-features -->
```text
       ╔═══════════════════╗
       ║ trigger           ║
       ║                   ║
       ║ [shared features] ║
       ║         │         ║
       ╚═════════╪═════════╝
           "new unit of"
                 │
                 │
                 │
           ╔═════╪═══════╗
           ║ observed by ║
           ║     ↓       ║
           ║ [shared]    ║
     ┌─────╫──┌──└──────┐║
     │     ╚══╪═════════╪╝
     │        │         │
     │        │         │
     │        │         │
     │        │         │
╔════╪════════╪═════════╪════╗
║ recompiles  │         │    ║
║    ↓        ↓         ↓    ║
║ [alpha]  [beta]   [shared] ║
║                            ║
╚════════════════════════════╝
  change:   +extra
  evidence: new unit hashes in the last 24h for shared
  hint:     feature unification depends on which packages a command selects
            unify per workspace: `[resolver] feature-unification = "workspace"` in .cargo/config.toml (nightly also needs `[unstable] feature-unification = true`), or enable the feature in every dependent.
```
<!-- /showcase -->

The output above comes from running these commands on this workspace; `cargo xtask sync-showcase` regenerates it.
