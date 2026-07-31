# AppSpec Parser

Parser and validator for the AWS CodeDeploy AppSpec YAML file.

## Overview

This module parses and validates AppSpec files for EC2/On-Premises deployments,
providing type-safe access to the deployment's hooks, file mappings, and
permissions. Parsing rejects malformed or contradictory specifications up front
so that the rest of the agent only ever sees a valid AppSpec.

**Entry point**: `AppSpec::parse(yaml_string)`

## Module Structure

```
application_specification/
├── mod.rs              # Public API exports
├── error.rs            # Error types (23 variants)
├── types.rs            # Core types (AppSpec, Version, Os, FileExistsBehavior)
├── parse.rs            # Main parsing logic
├── hooks.rs            # ScriptInfo, Timeout
├── files.rs            # FileMapping
├── mode.rs             # Unix mode bits (0-7777)
├── permissions.rs      # Permission validation
├── acl.rs              # POSIX ACL parsing
├── selinux.rs          # SELinux context and MLS range
└── pattern.rs          # Glob pattern matching
```

Each file carries its own `#[cfg(test)] mod tests`.

## Key Design Principles

1. **Parse → Validate → Construct**: invalid states are unrepresentable
2. **Type Safety**: enums for fixed values, newtypes for validated strings
3. **Memory Safety**: `#![forbid(unsafe_code)]`
4. **Stable Error Messages**: error text is part of the operator-facing contract
   and surfaces in the agent log and the CodeDeploy console

## Example Usage

```rust
use codedeploy_agent::application_specification::AppSpec;

let yaml = r#"
version: 0.0
os: linux
hooks:
  ApplicationStart:
    - location: scripts/start.sh
      timeout: 300
files:
  - source: /app
    destination: /var/www/app
"#;

let spec = AppSpec::parse(yaml)?;
assert_eq!(spec.version().as_f64(), 0.0);
assert_eq!(spec.os().as_str(), "linux");
```

## Supported Features

- **Version**: 0.0 only
- **OS**: Linux, Windows
- **Hooks**: All 6 lifecycle events (ApplicationStop, DownloadBundle, BeforeInstall, AfterInstall, ApplicationStart, ValidateService)
- **Files**: Source-to-destination mappings
- **Permissions** (Linux only):
  - Owner/group
  - Mode (octal 0-7777)
  - POSIX ACLs (base and default)
  - SELinux context with MLS range
- **File Exists Behavior**: DISALLOW, OVERWRITE, RETAIN

## Validation Rules

### Windows Restrictions
- Permissions section not allowed on Windows
- Error: `PermissionsOnWindows`

### File-Specific Constraints
- Pattern must be `**` (match all)
- No `except` patterns allowed
- No default ACLs allowed
- Enforced in `permissions.rs`, at apply time rather than parse time: these
  constraints only apply once a permission resolves to an actual copied file, so
  a directory `object:` with `type: [file]` plus a `pattern:`/`except:` is valid
  input and must parse.

### Timeout Validation
- Must be > 0
- Default: 3600 seconds
- Enforced in `hooks.rs`

### Mode Validation
- 1-4 octal digits (0-7)
- Padded to 3 digits minimum
- Range: 0-7777
- Enforced in `mode.rs`

### ACL Validation
- Format: `[default:]type:name:permissions`
- Types: user, group, mask, other
- Permissions: numeric (0-7) or symbolic (rwx)
- Base ACLs require names (except mask/other)
- Enforced in `acl.rs`

### SELinux Validation
- Format: `s#[-s#][:c#[.c#](,c#[.c#])*]`
- Sensitivity: s0-s15 (high >= low)
- Categories: c0-c1023 (no duplicates)
- Enforced in `selinux.rs`

## Testing

```bash
# Run all tests
cargo test

# Run only this module's tests
cargo test application_specification
```

## References

- [AppSpec file reference](https://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file.html)
- [AppSpec file example](https://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file-example.html)
