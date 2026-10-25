# cargo-frequent

Find out why Cargo keeps recompiling your crates. `cargo frequent` reads what Cargo recorded in `target/` and draws, for each cause, what triggers what to recompile, with a hint on how to stop it.

## Installation

```bash
cargo install cargo-frequent
```

## Usage

```bash
cargo frequent                          # read target/ only, nothing is built
cargo frequent --build                  # run `cargo check` first and include what it recompiled
cargo frequent --build -- test -p foo   # use another cargo command for --build
cargo frequent --json                   # the same graph as JSON
```

Without `--build` nothing is compiled, so the evidence of the last rebuild stays in place. It finds:

- **environment drift**: a build script watches a variable whose value differs from the one it last ran with, so the next build reruns it;
- **unit churn**: a crate got several unit hashes in the last 24 hours, for example because different commands selected different features, profiles or compilers.

`--build` adds the reasons Cargo logs while it decides what to recompile, such as changed files and `RUSTFLAGS`.

## Reading the graph

Every cause is one graph with three boxes:

- **trigger**: what changed, for example `$BINDGEN_EXTRA_CLANG_ARGS` or `shared features`;
- **observed by**: the crates that notice the change directly; `(build script)` marks a build script that watches it;
- **recompiles**: your workspace members that recompile as a result, with the number of crates in between in the box title.

Under the graph, `change` shows only the part that differs, `evidence` says where the finding comes from, and `hint` suggests a fix.

## Example

A Nix dev shell re-seeds `-frandom-seed` in `BINDGEN_EXTRA_CLANG_ARGS` each time it is evaluated, and every bindgen build script watches that variable:

<!-- showcase:nix-random-seed-drift -->
```text
╔═════════════════════════════╗
║ trigger                     ║
║                             ║
║ [$BINDGEN_EXTRA_CLANG_ARGS] ║
║             ┌┘              ║
╚═════════════╪═══════════════╝
        "watched by"
              │
              │
              │
╔═════════════╪═════════════╗
║ observed by │             ║
║             ↓             ║
║ [bindings (build script)] ║
║       ┌─────└────┐        ║
╚═══════╪══════════╪════════╝
        │          │
        │          │
        │          │
        │          │
    ╔═══╪══════════╪═════╗
    ║ recompiles   │     ║
    ║   ↓          ↓     ║
    ║ [app]   [bindings] ║
    ║                    ║
    ╚════════════════════╝
  change:   -frandom-seed=p11zjgl8gg -> -frandom-seed=f020dci6a9
  evidence: build scripts last ran 0s ago with the old value; the next build reruns them
  hint:     Nix picks a new -frandom-seed whenever the dev shell is re-evaluated
            strip it in shellHook: export BINDGEN_EXTRA_CLANG_ARGS="$(printf '%s' "$BINDGEN_EXTRA_CLANG_ARGS" | sed -E 's| ?-frandom-seed=[^ ]+||g')"
```
<!-- /showcase -->

More cases, each a real workspace with its own README, are in [`showcase/`](showcase/README.md).
