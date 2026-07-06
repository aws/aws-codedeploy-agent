# AWS CodeDeploy Agent

The AWS CodeDeploy agent is a software package that, when installed and
configured on an instance, makes it possible for that instance to be used in
[AWS CodeDeploy](https://aws.amazon.com/codedeploy/) deployments. The agent
receives deployment instructions from the CodeDeploy service, downloads your
application revisions, and runs the lifecycle event hooks defined in your
[AppSpec file](https://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file.html)
to install and validate your application on EC2/On-Premises instances.

The CodeDeploy agent is required only if you deploy to an EC2/On-Premises
compute platform. It is not required for deployments that use the Amazon ECS or
AWS Lambda compute platform.

The agent is licensed under [Apache License 2.0](LICENSE).

## Getting Started

### Prerequisites

- Rust (the minimum supported version is declared as `rust-version` in
  [`Cargo.toml`](Cargo.toml))
- Cargo (ships with Rust)

### Build from source

```bash
git clone https://github.com/aws/aws-codedeploy-agent.git
cd aws-codedeploy-agent
cargo build --release
```

The agent binary is placed at `target/release/codedeploy-agent`.

Windows builds are also supported via the `x86_64-pc-windows-msvc` and
`x86_64-pc-windows-gnu` targets. See [DEVELOPMENT.md](DEVELOPMENT.md) for the
toolchain setup.

### Run the agent

```bash
# Start the daemon
codedeploy-agent start

# Check status (exit 0 = running, exit 3 = stopped)
codedeploy-agent status

# Stop the daemon
codedeploy-agent stop
```

The agent reads its configuration from a YAML file; a documented template is
provided in [`conf/codedeployagent.yml`](conf/codedeployagent.yml) (installed
as the live config by the packages). Pass a custom config with
`--config-file <path>`.

## Documentation

For full guidance on installing, configuring, and operating the agent, see the
AWS CodeDeploy User Guide:

- [Working with the CodeDeploy agent](https://docs.aws.amazon.com/codedeploy/latest/userguide/codedeploy-agent.html)
  — overview, supported operating systems, communication protocol, and version
  history
- [Managing CodeDeploy agent operations](https://docs.aws.amazon.com/codedeploy/latest/userguide/codedeploy-agent-operations.html)
  — verify, determine version, install, update, and uninstall
- [Install the CodeDeploy agent](https://docs.aws.amazon.com/codedeploy/latest/userguide/codedeploy-agent-operations-install.html)
- [Install the CodeDeploy agent using AWS Systems Manager](https://docs.aws.amazon.com/codedeploy/latest/userguide/codedeploy-agent-operations-install-ssm.html)
- [Install the CodeDeploy agent using the command line](https://docs.aws.amazon.com/codedeploy/latest/userguide/codedeploy-agent-operations-install-cli.html)
- [CodeDeploy agent configuration reference](https://docs.aws.amazon.com/codedeploy/latest/userguide/reference-agent-configuration.html)

## Development

Common tasks are exposed through `make`:

```bash
make help     # List all available targets
make build    # Build the project
make test     # Run the test suite
make ci       # Run all CI checks (format, lint, test)
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for the full development workflow, tooling,
project layout, and local/end-to-end testing instructions.

## Contributing

Contributions are welcome. Please read the
[contribution guidelines](CONTRIBUTING.md) before opening a pull request, and
see [DEVELOPMENT.md](DEVELOPMENT.md) for development setup.

## Security

If you discover a potential security issue in this project, please do **not**
create a public GitHub issue. Instead, refer to the
[AWS Vulnerability Reporting](https://aws.amazon.com/security/vulnerability-reporting/)
page or email [aws-security@amazon.com](mailto:aws-security@amazon.com).

## Code of Conduct

This project has adopted the
[Amazon Open Source Code of Conduct](CODE_OF_CONDUCT.md).

## License

This project is licensed under the [Apache License 2.0](LICENSE). See the
[NOTICE](NOTICE) file for attribution.
