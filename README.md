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

## How

Under the hood it runs `cargo check` with `CARGO_LOG=cargo::core::compiler::fingerprint=info`, parses the fingerprint log lines that explain why each unit is dirty, and groups the results by their underlying trigger (env var, file, rustflags, …). For env-var triggers it cross-references the `cargo:rerun-if-env-changed` directives emitted by every build script in `target/<profile>/build/*/output`, so you can see which crate is actually watching the variable.

Each cluster is rendered as a Sugiyama-layered ASCII DAG with three tiers — the trigger at the top, the source crates whose units cargo flagged or whose build scripts watch the trigger in the middle, and downstream crates that rebuilt transitively at the bottom. Edges are labeled with the relationship.

*Volatile* means the env var is on a curated list (`PATH`, `HOME`, `XDG_*`, `CONDA_*`, `SSH_*`, …) of variables that typically vary between shells/sessions, so anything watching them is liable to rebuild on every invocation.

Use `--json` for the same data as structured output, or `--command` to analyze something other than `check` (e.g. `--command build`).

## Example

Suppose you maintain a Rust project with Python bindings via `pyo3` and a `numpy`-backed extension. Every time you switch shells, your `PATH` environment variable changes (a common annoyance with virtualenvs and direnv). The `pyo3-build-config` build script declares `cargo:rerun-if-env-changed=PATH`, so cargo re-runs it on the next invocation; that re-run changes its fingerprint, which forces `numpy` (and its build script) to be re-checked, which in turn forces your own `python_utils` crate to be re-checked. Running `cargo frequent` after such an invocation prints:

```
       [$PATH (volatile)]
                │ watched by
   [pyo3-build-config (build script)]
                │ transitive
            [numpy]
                │ transitive
        [python_utils]
```

Read top-to-bottom: `$PATH` changed (and is *volatile*, so this is likely to happen often); `pyo3-build-config`'s build script watches it; that triggered `numpy` to be re-checked transitively; which then triggered `python_utils`.

## Installation

Installation:

```bash
cargo install cargo-frequent
```
