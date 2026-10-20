# nullidentd

A dummy ident (RFC 1413) daemon written in async Rust. It responds to every
ident query with a fixed, configurable username — useful for satisfying servers
that require an ident response without exposing real user information.
It does not look up connection owners, and its replies must not be used for
authentication or access control.

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
nullidentd --listen '[::]:1113,0.0.0.0:1113'
```

Every address returned when resolving a hostname is bound. IPv6 listeners are
IPv6-only, so the example above also needs its IPv4 listener. All required
listeners must bind successfully before serving begins; startup failures and
fatal listener failures produce a nonzero exit status. Use `--verbose` to see
the actual bound addresses, including assigned ports when listening on port 0.

The `--timeout` value is in seconds, must be between 1 and 3600, and limits the
total connection lifetime. The default is 5 seconds. Additional queries do not
extend this deadline.

The daemon handles at most 64 connections across all listeners. Excess
connections are closed without a response. The process file-descriptor limit
must allow additional room for listeners, runtime resources, and pending
accepts. Temporary accept failures, including descriptor exhaustion, are
retried with backoff and rate-limited error logging.

The `--ident` value sets the username returned in every response. For example,
`--ident nobody` will cause the daemon to reply with
`<port-pair> : USERID : UNIX :nobody\r\n`. No extra space is inserted before
the username. Ident values must contain 1–512 printable ASCII bytes.

## Protocol handling and logging

Requests must end in CRLF, contain two decimal ports from 1 through 65535,
and fit within 1024 bytes including CRLF. Spaces and tabs around ports and
the comma are accepted. Port fields must contain one through five ASCII digits.
Several queries may be sent on one connection until its lifetime expires.

Recognizable numeric port pairs with invalid ports receive `ERROR : INVALID-PORT`.
Other malformed input, incomplete requests, and oversized requests cause the
connection to close without a response. HTTP probes are not echoed.

Logs go to stderr. Routine connection events, client timeouts, malformed probes,
and client I/O failures are logged only with `--debug` or `--trace`; request and
response payloads are not logged. Operational accept failures are logged at
most once per 30 seconds per listener.

## Shutdown

Ctrl-C (`SIGINT`) and, on Unix, `SIGTERM` use the same controlled shutdown:
listeners close, idle connections close, and responses already being written
may finish. Remaining connection tasks are aborted after a five-second grace
period. A second shutdown signal skips the remaining grace period. Fatal
listener failures use the same cleanup path and return a nonzero exit status.

## License

MIT
