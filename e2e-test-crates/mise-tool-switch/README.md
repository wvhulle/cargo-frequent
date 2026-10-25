# mise switches a tool version on `PATH`

`protobuf-codegen` watches `PATH` to find `protoc`, like `prost-build`. The project pins `protoc` with mise (`mise.toml`), and mise puts the selected version's directory on `PATH`. Changing the pinned version, or opening a shell where mise is not active, reruns the build script.

```sh
cargo check                 # with protoc 27.0 from mise
# mise.toml now pins protoc 28.1
cargo frequent
```

<!-- e2e:mise-tool-switch -->
```text
1 cause recompiles 2 of 2 workspace members (last 24h)

1  $PATH   mise.toml  (recorded 0s ago, reruns on the next build)
├── change
│   ├── removed     /home/user/.local/share/mise/installs/protoc/27.0/bin
│   └── added       /home/user/.local/share/mise/installs/protoc/28.1/bin
├── set by
│   └── manager       mise.toml  mise puts the selected tool versions on PATH
├── watched by  protobuf-codegen (build script)
├── recompiles  all 2 workspace members
└── fix         mise.toml  the mise tool version on PATH changed
                one rebuild is expected after editing it
                if it keeps flipping, activate mise in every shell that runs cargo,
                or point the build script at the tool path directly
```
<!-- /e2e -->

cargo-frequent recognises the mise install directory in the changed `PATH` entries and names `mise.toml` as the source.

The steps live in [`scenario.toml`](scenario.toml). `cargo test --test e2e_crates` checks that the output above is still what `cargo frequent` prints, and `cargo xtask update-e2e` rewrites it.
