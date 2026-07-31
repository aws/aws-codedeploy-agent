# Contributing to codedeploy-agent

Thank you for your interest in contributing to the project.

## Setting up the package

This project builds with standard [Cargo](https://doc.rust-lang.org/cargo/). After
cloning, build it with:

```console
$ cargo build
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for the full development workflow, tooling, and
available `make` targets.

## Writing code

### Code style

This project follows the standard conventions for Rust projects imposed by
[`rustfmt`](https://github.com/rust-lang/rustfmt). `rustfmt` is exposed via the
`cargo fmt` sub-command.

```console
$ cargo fmt
```

To assist with writing idiomatic code, you should also regularly apply the `clippy`
code linter. This can also be invoked by `cargo`:

```console
$ cargo clippy
```

### Dependencies

To add a dependency, add it to the package's `Cargo.toml` file.
