# filerep

Simple command line tool to track incremental file changes and send them to recipients over TCP.

Server cmd: `cargo run -- --server <port> <path>`

Client cmd: `cargo run -- <port> <path> <address>`