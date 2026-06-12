# nullidentd

A dummy ident (RFC 1413) daemon written in async Rust. It responds to every
ident query with a fixed, configurable username — useful for satisfying servers
that require an ident response without exposing real user information.

## Building

```sh
cargo build --release
```

A size-optimized profile is also available:

```sh
cargo build --profile minsize
```

## Development

```sh
cargo build
cargo test
cargo fmt
cargo clippy --all-targets --all-features
cargo outdated --root-deps-only
```

Use `cargo test` for the built-in Rust test suite, `cargo fmt` before
committing, and `cargo clippy --all-targets --all-features` for lint checks.
Use `cargo outdated --root-deps-only` when checking whether direct dependencies
can be updated.

## Usage

```
Usage: nullidentd [OPTIONS]

Options:
  -v, --verbose
  -d, --debug
  -t, --trace
  -l, --listen <LISTEN>...  [default: localhost:1113]
      --timeout <TIMEOUT>   [default: 5]
      --ident <IDENT>       [default: user]
  -h, --help                Print help
```

Multiple listen addresses can be given as a comma-delimited list, for example
in dual-stack scenarios:

```sh
nullidentd --listen [::]:1113,0.0.0.0:1113
```

The `--timeout` value is in seconds and applies per connection. Connections
that do not complete within the timeout are dropped.

The `--ident` value sets the username returned in every response. For example,
`--ident nobody` will cause the daemon to reply with
`<port-pair> : USERID : UNIX : nobody`.

## License

MIT
