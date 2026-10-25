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
cargo frequent --all                    # also show one-off causes and causes beyond the top 5
cargo frequent --graph                  # draw each cause as a boxed graph
cargo frequent --json                   # the same report as JSON
```

Without `--build` nothing is compiled, so the evidence of the last rebuild stays in place. It finds:

- **environment drift**: a build script watches a variable whose value differs from the one it last ran with, so the next build reruns it;
- **unit churn**: crates got several unit hashes in the last 24 hours, for example because different commands selected different features, profiles or compilers.

`--build` adds the reasons Cargo logs while it decides what to recompile, such as changed files and `RUSTFLAGS`.

One-off causes, such as a `Cargo.lock` bump or Nix store paths that changed after a flake update, rebuild once and stay quiet afterwards. They are hidden unless you pass `--all`.

## Reading the output

The first line sums up how many causes recompile how many of your workspace members. Then each cause gets one numbered tree, most actionable first:

- the **heading** names the trigger and the evidence (`recorded 3h ago`, `113 crates got new unit hashes`);
- **change** shows only the part that differs;
- **watched by** / **flips in** / **rebuilt for** lists the crates that notice the change directly; `(build scripts)` marks build scripts;
- **recompiles** lists the workspace members that recompile as a result, and how many crates sit in between;
- **fix** is a suggestion you can usually paste.

## Example

A Nix dev shell re-seeds `-frandom-seed` in `BINDGEN_EXTRA_CLANG_ARGS` each time it is evaluated, and every bindgen build script watches that variable:

<!-- showcase:nix-random-seed-drift -->
```text
1 cause recompiles 2 of 2 workspace members (last 24h)

1  $BINDGEN_EXTRA_CLANG_ARGS   (recorded 0s ago, reruns on the next build)
   ├─ change      -frandom-seed=p11zjgl8gg -> -frandom-seed=f020dci6a9
   ├─ watched by  bindings (build script)
   ├─ recompiles  all 2 workspace members
   └─ fix         Nix picks a new -frandom-seed whenever the dev shell is re-evaluated
                  strip it in shellHook: export BINDGEN_EXTRA_CLANG_ARGS="$(printf '%s' "$BINDGEN_EXTRA_CLANG_ARGS" | sed -E 's| ?-frandom-seed=[^ ]+||g')"
```
<!-- /showcase -->

More cases, each a real workspace with its own README, are in [`showcase/`](showcase/README.md).
