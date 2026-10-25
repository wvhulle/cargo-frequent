# A build script watches `PATH`

`python-config` watches `PATH`, as `pyo3-build-config` does when `PYO3_PYTHON` is unset. Activating a virtualenv, or any shell that prepends to `PATH`, reruns the build script.

```sh
cargo check
PATH="/home/user/.venv/bin:$PATH" cargo frequent
```

<!-- showcase:volatile-path-watch -->
```text
1 cause recompiles 2 of 2 workspace members (last 24h)

1  $PATH   (recorded 0s ago, reruns on the next build)
   ├─ change      added /home/user/.venv/bin
   ├─ watched by  python-config (build script)
   ├─ recompiles  all 2 workspace members
   └─ fix         PATH differs between shells by nature
                  pin the tool the build script is looking for (e.g. PYO3_PYTHON) so it stops watching this variable
```
<!-- /showcase -->

The output above comes from running these commands on this workspace; `cargo xtask sync-showcase` regenerates it.
