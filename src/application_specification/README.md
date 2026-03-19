# AppSpec Parser

Rust implementation of the AWS CodeDeploy AppSpec YAML parser, migrated from the Ruby CodeDeploy agent.

## Overview

This module parses and validates AppSpec files for EC2/On-Premises deployments. It provides type-safe parsing with comprehensive validation matching the original Ruby implementation exactly.

**Entry Point**: `AppSpec::parse(yaml_string)`

## Ruby to Rust Mapping

| Ruby File | Rust File(s) | Description |
|-----------|--------------|-------------|
| `application_specification.rb` | `parse.rs`, `types.rs`, `mod.rs` | Main parser and core types |
| `script_info.rb` | `hooks.rs` | Lifecycle hook scripts |
| `file_info.rb` | `files.rs` | File mappings (source → destination) |
| `mode_info.rb` | `mode.rs` | Unix file permissions (octal mode bits) |
| `ace_info.rb` | `acl.rs` | POSIX ACL entries |
| `acl_info.rb` | `acl.rs` | POSIX ACL collections |
| `linux_permission_info.rb` | `permissions.rs` | Permission validation and constraints |
| `context_info.rb` | `selinux.rs` | SELinux security context |
| `range_info.rb` | `selinux.rs` | SELinux MLS range |
| *(Ruby inline)* | `pattern.rs` | Glob pattern matching |
| *(Ruby inline)* | `error.rs` | Error types with exact Ruby messages |

## Module Structure

```
appspec/
├── mod.rs              # Public API exports
├── error.rs            # 23 error types
├── types.rs            # Core types (AppSpec, Version, Os, etc.)
├── parse.rs            # Main parsing logic
├── hooks.rs            # ScriptInfo, Timeout
├── files.rs            # FileMapping
├── mode.rs             # Unix mode bits (0-7777)
├── permissions.rs      # Permission validation
├── acl.rs              # POSIX ACL parsing
├── selinux.rs          # SELinux context and MLS range
├── pattern.rs          # Glob pattern matching
└── *_tests.rs          # Test files (per Rust conventions)
```

## Key Design Principles

1. **Parse → Validate → Construct**: Invalid states are unrepresentable
2. **Type Safety**: Enums for fixed values, newtypes for validated strings
3. **Memory Safety**: `#![forbid(unsafe_code)]`
4. **Error Compatibility**: All error messages match Ruby exactly
5. **Test Coverage**: 191 unit tests + 6 integration tests = 91.7% coverage

## Example Usage

```rust
use aws_codedeploy_agent::appspec::AppSpec;

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
- Enforced in `permissions.rs`

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

# Current metrics
# Tests: 198 (191 unit + 6 integration + 1 lib)
# Coverage: 91.7% (510/556 lines)
# Warnings: 0
```

## References

- **Ruby Source**: `aws-codedeploy-agent/lib/instance_agent/plugins/codedeploy/application_specification/`
- **Design Doc**: `BoltCommonContext/CodeDeploy/Agent/Migrations/application-specification/rust-design.md`
- **AWS Docs**: https://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file-example.html
