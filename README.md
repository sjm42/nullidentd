# nullidentd
Dummy identd in Rust

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

Please note: multiple listen addresses can be given, for example in dual stack scenarios.
Use a comma-delimited list, like `--listen [::]:1113,0.0.0.0:1113`
