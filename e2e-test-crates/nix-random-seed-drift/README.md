# Nix dev shell re-seeds `-frandom-seed`

`bindings` has a build script that watches `BINDGEN_EXTRA_CLANG_ARGS`, like every `bindgen`-based `-sys` crate. The project uses direnv (`.envrc`: `use flake`), so that variable comes from the Nix dev shell, and nixpkgs ends it with `-frandom-seed=<first 10 characters of the dev shell's store hash>`. Any edit to `flake.nix` or `flake.lock` gives the dev shell a new hash, so the next build reruns the build script and recompiles everything above it.

```sh
echo "use flake" > .envrc
BINDGEN_EXTRA_CLANG_ARGS="… -frandom-seed=p11zjgl8gg" cargo check
touch flake.nix   # direnv re-evaluates the dev shell: new store hash, new seed
cargo frequent
```

<!-- e2e:nix-random-seed-drift -->
```text
1 cause recompiles 2 of 2 workspace members (last 24h)

1  $BINDGEN_EXTRA_CLANG_ARGS   flake.nix:10  (recorded 0s ago, reruns on the next build)
├── change      -frandom-seed=p11zjgl8gg -> -frandom-seed=f020dci6a9
├── set by
│   ├── direnv        .envrc:1  use flake
│   ├── dev shell     flake.nix:10  /nix/store/f020dci6a9…-nix-shell-env
│   │                 the new seed f020dci6a9 is the start of this store hash
│   │                 (nixpkgs seeds the compiler with it, so every change to the shell changes the seed)
│   └── changed       flake.nix  modified after the build scripts last ran
├── watched by  bindings (build script)
├── recompiles  all 2 workspace members
└── fix         flake.nix:10  pin the seed in the dev shell:
                env.NIX_OUTPATH_USED_AS_RANDOM_SEED = "dev-shell";
                (nixpkgs' reproducible-builds hook then seeds with "dev-shell" instead of the store hash)
                docs: https://github.com/NixOS/nixpkgs/blob/8edc0c72e3a38faf5434e40d1f19431125fe4b30/pkgs/build-support/setup-hooks/reproducible-builds.sh#L5-L6
```
<!-- /e2e -->

The scenario writes `.envrc` and the nix-direnv profile in `.direnv/` (an `.rc` file plus the symlink to the dev shell's store path) into a temporary copy, so this repository does not ask direnv to load them.

The steps live in [`scenario.toml`](scenario.toml). `cargo test --test e2e_crates` checks that the output above is still what `cargo frequent` prints, and `cargo xtask update-e2e` rewrites it.
