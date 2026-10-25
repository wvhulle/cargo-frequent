# Nix re-seeds `-frandom-seed`

`bindings` has a build script that watches `BINDGEN_EXTRA_CLANG_ARGS`, like every `bindgen`-based `-sys` crate. In a Nix dev shell that variable ends with `-frandom-seed=<hash>`, and nixpkgs picks a new hash every time the dev shell is evaluated again (`direnv reload`, `nix develop`, an editor starting its own shell). The build script reruns and everything above it recompiles, although nothing you care about changed.

```sh
BINDGEN_EXTRA_CLANG_ARGS="… -frandom-seed=p11zjgl8gg" cargo check
BINDGEN_EXTRA_CLANG_ARGS="… -frandom-seed=f020dci6a9" cargo frequent
```

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

The output above comes from running these commands on this workspace; `cargo xtask sync-showcase` regenerates it.
