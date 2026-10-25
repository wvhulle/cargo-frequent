# cargo `[env]` changes a watched variable

`openssl-bindings` watches `OPENSSL_DIR`. The project sets it in `.cargo/config.toml` under `[env]`, and cargo passes those values to every build script. Editing the value reruns the build script, even though no shell variable changed.

```sh
cargo check                 # OPENSSL_DIR = "/opt/openssl-3.0" in .cargo/config.toml
# .cargo/config.toml now sets "/opt/openssl-3.3"
cargo frequent
```

<!-- e2e:cargo-config-env -->
```text
1 cause recompiles 2 of 2 workspace members (last 24h)

1  $OPENSSL_DIR   .cargo/config.toml:2  (recorded 0s ago, reruns on the next build)
├── change      /opt/openssl-3.0 -> /opt/openssl-3.3
├── set by
│   └── cargo [env]   .cargo/config.toml:2  OPENSSL_DIR = "/opt/openssl-3.3"
├── watched by  openssl-bindings (build script)
├── recompiles  all 2 workspace members
└── fix         .cargo/config.toml:2  OPENSSL_DIR comes from cargo [env] and its value changed
                one rebuild is expected after editing it
                if it keeps flipping, another tool passes a different value
                (environment variable or --config 'env.OPENSSL_DIR=…'): make it use the same one
                docs: https://doc.rust-lang.org/cargo/reference/config.html#env
```
<!-- /e2e -->

cargo-frequent reads the configured value (the shell does not set `OPENSSL_DIR`) and points at the config line.

The steps live in [`scenario.toml`](scenario.toml). `cargo test --test e2e_crates` checks that the output above is still what `cargo frequent` prints, and `cargo xtask update-e2e` rewrites it.
