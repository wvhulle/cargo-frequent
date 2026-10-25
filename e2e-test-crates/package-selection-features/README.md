# Package selection changes features

`alpha` enables the `extra` feature of `shared`, `beta` does not, and `default-members` only lists `beta`. Cargo unifies features over the packages a command selects, so `cargo build` (only `beta`) and `cargo check -p alpha` compile two different `shared` units, and switching between them keeps recompiling everything above `shared`.

```sh
cargo check -p beta
cargo check -p alpha
cargo frequent
```

<!-- e2e:package-selection-features -->
```text
1 cause recompiles 3 of 3 workspace members (last 24h)

1  feature selection   Cargo.toml:3  (1 crate got new unit hashes in the last 24h)
├── change      shared +extra
├── why
│   └── toggled by    shared/extra
│                     on:  alpha
│                     off: beta or shared built alone
├── recompiles  all 3 workspace members
└── fix         Cargo.toml:3  plain `cargo build` builds only default-members, without shared/extra
                while alpha turns it on
                build with --workspace every time,
                or unify features for the whole workspace in .cargo/config.toml:
                [resolver] feature-unification = "workspace"
                (nightly only: stable cargo ignores it; also set [unstable] feature-unification = true)
                docs: https://doc.rust-lang.org/cargo/reference/unstable.html#feature-unification
```
<!-- /e2e -->

cargo-frequent names the member that turns the feature on and the default build that leaves it off.

The steps live in [`scenario.toml`](scenario.toml). `cargo test --test e2e_crates` checks that the output above is still what `cargo frequent` prints, and `cargo xtask update-e2e` rewrites it.
