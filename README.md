# cargo-frequent

This tool tells the cause of frequent recompilations by Cargo in Rust projects. It uses by default the verbose output of the Cargo `check` command. Causes of frequent recompiles that this tool may find:

- environment variable changes (most common)
- file changes
- Rust compiler flags changed
- dependency changed
- feature changed
- ...

## Usage

Just run this command:

```bash
cargo frequent
```

Under the hood it runs `cargo check` with `CARGO_LOG=cargo::core::compiler::fingerprint=info`, parses the fingerprint log lines that explain why each unit is dirty, and groups the results by their underlying trigger (env var, file, rustflags, …). For env-var triggers it cross-references the `cargo:rerun-if-env-changed` directives emitted by every build script in `target/<profile>/build/*/output`, so you can see which crate is actually watching the variable.

Each cluster is rendered as a Sugiyama-layered ASCII DAG with the trigger at the top and every package it caused to rebuild fanning out below:

```
 [env:PATH [volatile] watched-by:pyo3-build-config]
              |
       +------+------+
       |             |
    [numpy]   [python_utils]
```

`[volatile]` marks env vars on the curated list (PATH, HOME, XDG_*, …) that vary between shells/sessions and so guarantee rebuilds. Use `--json` for the same data as structured output, or `--command` to analyze something other than `check` (e.g. `--command build`).

## Installation

Installation:

```bash
cargo install cargo-frequent
```

 If something does not work for you, please create a bug report in the source repository.
