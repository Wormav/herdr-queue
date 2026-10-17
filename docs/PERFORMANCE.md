# Native port measurements

Herdr Queue 0.2.0 replaces the Python/Textual application with a Rust executable using
Ratatui, Crossterm, ratatui-textarea, and bundled SQLite. The plugin ID, state directory,
database schema, target keys, and manual submission safeguards remain compatible.

## Local comparison

Measured on macOS Apple Silicon in a 120×40 pseudo-terminal. Both applications opened
their isolated demo with the same two prompts, remained open for seven seconds, then
closed with Ctrl+Q. Neither contacted an agent. Memory was collected from the child's
`getrusage(RUSAGE_CHILDREN).ru_maxrss`; on macOS this field is reported in bytes.

| Measurement | Python 0.1.0 | Native Rust 0.2.0 |
| --- | ---: | ---: |
| Peak resident memory | 49.69 MiB | 4.77 MiB |
| Total process CPU time, including startup | 0.3752 s | 0.0173 s |
| Time until exit, including startup and shutdown | 7.56 s | 7.09 s |

The measured native peak was about **90% lower**. The stripped macOS ARM64 executable
was **1.82 MiB**. The release profile uses size optimization, link-time optimization,
and one code generation unit; SQLite is included in the executable.

These figures concern the Queue process, excluding the terminal, Herdr, and the AI CLI.
They are one short comparison, not a long-session or large-history benchmark, and are
not an allocation limit. Resident memory and macOS Activity Monitor's memory footprint
are different accounting metrics.

## Check a local build

```sh
cargo build --release --locked
/usr/bin/time -l target/release/herdr-queue demo
```

Use a 120×40 terminal, wait seven seconds, then close with Ctrl+Q. On macOS, inspect
`maximum resident set size`, `user time`, and `system time`. On Linux, use
`/usr/bin/time -v` and note that maximum resident memory is reported in KiB.

The interface redraws after changes. Between events, it waits for terminal input or a
status refresh deadline; it does not render continuously. There is at most one network
worker per view, and the plugin does not run a background service after closing.
