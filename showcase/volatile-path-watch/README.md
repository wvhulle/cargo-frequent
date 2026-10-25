# A build script watches `PATH`

`python-config` watches `PATH`, as `pyo3-build-config` does when `PYO3_PYTHON` is unset. Activating a virtualenv, or any shell that prepends to `PATH`, reruns the build script.

```sh
cargo check
PATH="/home/user/.venv/bin:$PATH" cargo frequent
```

<!-- showcase:volatile-path-watch -->
```text
            ╔═════════╗
            ║ trigger ║
            ║         ║
            ║ [$PATH] ║
            ║    │    ║
            ╚════╪════╝
           "watched by"
                 │
                 │
                 │
╔════════════════╪═══════════════╗
║ observed by    │               ║
║                ↓               ║
║ [python-config (build script)] ║
║      ┌─────────└───┐           ║
╚══════╪═════════════╪═══════════╝
       │             │
       │             │
       │             │
       │             │
╔══════╪═════════════╪════════╗
║ recompiles         │        ║
║      ↓             ↓        ║
║ [extension] [python-config] ║
║                             ║
╚═════════════════════════════╝
  change:   added /home/user/.venv/bin
  evidence: build scripts last ran 0s ago with the old value; the next build reruns them
  hint:     PATH differs between shells by nature
            pin the tool the build script is looking for (e.g. PYO3_PYTHON) so it stops watching this variable
```
<!-- /showcase -->

The output above comes from running these commands on this workspace; `cargo xtask sync-showcase` regenerates it.
