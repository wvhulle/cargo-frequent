# Activating a Python virtualenv changes `PATH`

`python-config` watches `PATH`, as `pyo3-build-config` does when `PYO3_PYTHON` is unset. Activating a virtualenv prepends its `bin/` to `PATH`, so the build script reruns whenever you switch between a shell with and without the virtualenv.

```sh
cargo check
source /home/user/.venv/bin/activate
cargo frequent
```

<!-- e2e:virtualenv-path-watch -->
```text
1 cause recompiles 2 of 2 workspace members (last 24h)

1  $PATH   (recorded 0s ago, reruns on the next build)
├── change      added /home/user/.venv/bin
├── set by
│   └── activated     Python virtualenv /home/user/.venv
│                     activation prepends /home/user/.venv/bin to PATH
├── watched by  python-config (build script)
├── recompiles  all 2 workspace members
└── fix         .cargo/config.toml  activating the Python virtualenv changes PATH, which the build script watches
                pin the interpreter so the build script stops searching PATH:
                add `PYO3_PYTHON = "/home/user/.venv/bin/python"` under [env]
                docs: https://pyo3.rs/latest/building-and-distribution.html#configuring-the-python-version
```
<!-- /e2e -->

cargo-frequent recognises the virtualenv from `VIRTUAL_ENV` and suggests pinning the interpreter instead of watching `PATH`.

The steps live in [`scenario.toml`](scenario.toml). `cargo test --test e2e_crates` checks that the output above is still what `cargo frequent` prints, and `cargo xtask update-e2e` rewrites it.
