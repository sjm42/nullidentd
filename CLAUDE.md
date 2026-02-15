# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

nullidentd is a dummy ident (RFC 1413) daemon written in async Rust using Tokio. It listens on one or more TCP addresses and replies to every ident query with a fixed username (`USERID : UNIX : <ident>`).

## Build Commands

- `cargo build` — debug build
- `cargo build --release` — optimized release build (fat LTO, codegen-units=1)
- `cargo build --profile minsize` — size-optimized release build (stripped, panic=abort)
- `cargo fmt` — format code
- `cargo clippy` — lint

There are no tests in this project.

## Architecture

Small single-binary project with three source files:

- **`src/bin/nullidentd.rs`** — Entry point. Sets up a multi-threaded Tokio runtime, spawns one `run_server` task per listen address (via `JoinSet`), each accepting TCP connections and spawning per-connection tasks with a timeout.
- **`src/config.rs`** — CLI argument parsing via `clap` derive and tracing/log level setup. `OptsCommon::start_pgm()` initializes the tracing subscriber.
- **`src/lib.rs`** — Re-exports `clap::Parser`, `tracing::*`, and `config::*` so the binary can `use nullidentd::*`.

**`build.rs`** uses the `build-data` crate to embed git branch, commit, source timestamp, and rustc version as compile-time environment variables.

## Code Style

- Rust 2024 edition, stable toolchain
- rustfmt: 120 char line width, crate-level import granularity, imports grouped by std/external/crate
- Files end with `// EOF` comment
