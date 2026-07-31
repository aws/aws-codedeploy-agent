## 2.0.0

Version 2.0.0 replaces the Ruby runtime with a single statically linked binary in Rust, completely removing the dependency on a Ruby runtime.

The rewrite is behavior-compatible by default. Switching from v1.8.x to v2.0.0 is not expected to affect deployment outcomes, and all behaviors match the previous agent unless explicitly noted below. The new agent also introduces a set of opt-in configuration flags that let you tighten its security posture to match your environment's requirements.

### Added

* **Single native binary.** The 3-layer v1 entry point (version-detection shim, GLI CLI, SysV init.d script) is replaced by one executable with a `clap` CLI (`start`/`stop`/`restart`/`status`/`deploy-local`).
* **Local command port.** A loopback-only (127.0.0.1) TCP management interface exposing `ping`, `status`, and `inject` commands. Disabled by default (`enable_command_port`). Protected by a 256-bit CSPRNG token in a `0600` / SYSTEM+Administrators-only discovery file with constant-time comparison, an 8-connection cap, a 30s idle timeout, and a 64 KiB request limit.
* **Native Windows service integration.** Service install/uninstall/run via the Service Control Manager, replacing the `win32-daemon` gem and Ocra toolchain. Reports `StartPending` then `Running` so the SCM no longer races slow startup (error 1053), auto-detects whether it was launched by the SCM or a console, validates that the service binary lives under the expected install prefix, and stops the service before deleting it on uninstall.
* Custom WiX MSI manifest that installs to `C:\ProgramData\Amazon\CodeDeploy\` and registers the service against the correct entry point.
* **Concurrent deployments.** After dispatching a command, and if capacity remains, the agent re-polls immediately instead of waiting the full `wait_between_runs` interval (default 30s), enabling back-to-back command pickup during multiple deployments.
* **In-flight command de-duplication.** A command the service re-delivers while the agent is still processing it is skipped rather than run twice, with a bounded consecutive-skip cap before the agent falls back to its normal poll interval.
* **Process-wide throttle circuit breaker.** A shared backoff gate coordinates all worker threads on a single deadline when the service returns HTTP 429 (or non-429 throttling signals), and retries `PutHostCommandComplete` rather than dropping it.
* **Richer service-error diagnostics.** Non-2xx responses preserve the service `__type` and response body in agent logs instead of collapsing to a bare `HTTP <status>`.
* Startup TLS pre-check that fails fast on a broken endpoint/certificate before entering the poll loop.
* Structured per-hook audit log recording source user, target user (`runas`), `sudo`, and script path for every lifecycle hook spawn.
* A `.version` file written next to the install root at service startup (mode `0444`; protected DACL on Windows), reporting the running agent version.
* `s3://` and GitHub bundle sources for `deploy-local`. GitHub browser URLs (`https://github.com/<org>/<repo>`) and API URLs (`https://api.github.com/repos/<org>/<repo>/{zipball,tarball}/<ref>`) are both accepted. Private repositories use `--github-token`, and GitHub downloads honor `proxy_uri`.
* New configuration keys (all defaulting to v1-compatible behavior): `enable_command_port`, `reject_symlinks_in_bundle`, `reject_path_traversal_in_bundle`, `reject_unsafe_permissions_in_bundle`, `reject_unconfined_selinux_in_bundle`, `reject_symlink_permission_targets`, `ignore_ownership_in_bundle`, `restrict_agent_dir_permissions`, `restrict_log_dir_permissions`, `restrict_hook_env_to_allowlist`, `strip_loader_env_in_hooks`, `disable_powershell_profile_in_hooks`, `archive_max_extraction_size`, and `disable_core_dumps` (see "Changed" for its non-default value).

### Changed

* **Concurrency model.** The v1 fork-based process manager and thread pool are replaced by a master process that supervises a single worker child (spawned by re-executing the binary) plus an internal 16-way thread pool. The external contract (one master, one worker, PID file, SIGTERM stop) is unchanged.
* **Service management on Linux** uses a native systemd unit (`Type=simple`, `Restart=on-abnormal`, `TimeoutStopSec=7200`, control-group kill) instead of the SysV init.d script and `systemd-sysv-generator` compatibility layer.
* The systemd unit launches the agent through `bash -a -c`, so lifecycle hooks inherit the full service environment (including `BASH_EXECUTION_STRING`), and enables `MemoryAccounting` so `systemctl status` reports a Memory line.
* **Worker respawn uses exponential backoff** (5s to 60s, reset after 60s of healthy uptime) instead of a fixed 5s delay, avoiding a tight respawn loop on a persistently crashing worker.
* **AWS command client** is a purpose-built JSON-1.1/SigV4 implementation instead of the vendored SDK. Retry/backoff lives in the poll loop and the throttle gate; the client itself performs no hidden retries. Endpoint resolution, the exponential backoff formula, the agent-version header, and the 5-minute credential-refresh window all match v1.
* **In-place credential refresh.** Rotated credentials are picked up without restarting the agent or failing long-running deployments.
* **`disable_core_dumps` defaults to `true`.** The agent holds AWS credentials in memory, so core dumps are suppressed (`RLIMIT_CORE=0` + `PR_SET_DUMPABLE=0` on Linux, `SetErrorMode` on Windows). Set `disable_core_dumps: false` to restore the v1 behavior for crash investigation.
* **`deploy_control_endpoint` scheme validation.** Only `https://` and `http://` are accepted. All other schemes (`file:`, `javascript:`, `ftp:`, `gopher:`, `data:`) are rejected at config load. A custom `http://` endpoint is accepted (it is a deliberate operator choice, such as a loopback sidecar that terminates TLS) and logs a warning that credentials and command payloads transmit in cleartext.
* **The local deploy CLI** is now the `deploy-local` subcommand of the single binary instead of the separate `codedeploy-local` executable. All flags are preserved, and the documented `codedeploy-local [options]` invocation keeps working: the `.deb` and `.rpm` install a `codedeploy-local` symlink alongside the agent binary, which dispatches on `argv[0]`.
* **Agent state files and directories stay world-readable by default** (files `0644`, dirs `0755`), matching v1, because host tooling outside the agent reads them. Two opt-in flags tighten them: `restrict_agent_dir_permissions` (install root, per-deployment logs, and state files get dirs `0700`/`0711`/`0750`, log files `0640`, state files `0600`) and `restrict_log_dir_permissions` (log directory gets dir `0750`, files `0640`). On Windows, a protected DACL granting access to SYSTEM and Administrators only is applied to the `.version` file.
* **Agent self-update downloads from the `latestv2/` prefix** in the regional `aws-codedeploy-<region>` bucket instead of `latest/`, which stays frozen on the 1.x installer. Affects both the `update` subcommand and the service-driven `UpdateDeploymentAgent` host command. Hosts that mirror or allowlist the bucket path must permit `latestv2/install`.
* **The agent configuration ships at `conf/codedeployagent.yml`** and is installed as the live config (`/etc/codedeploy-agent/conf/codedeployagent.yml`), replacing the `codedeployagent.example.yml` template. The `.rpm` marks it `noreplace`, so an upgrade preserves local edits. Windows ships `conf/conf.yml`.
* File writes are atomic (temp file + rename) for crash safety; the observable result is unchanged.
* `status` returns LSB-conformant exit codes (3 = not running, 4 = unknown).
* The default `wait_between_runs` is 30 seconds (matching the v1 code default, not the 1-second fast-poll value in the v1 in-repo sample config).

### Fixed

* **AppSpec ACL synthesis** emitted two base entries in forms `setfacl --set` rejects: the base owner as `:<mode>` (rejected by strict libacl on Debian and Ubuntu) and the default owner as `d:<mode>` (rejected on every distribution). Both now use the canonical `u::<mode>` / `d::<mode>` forms, repairing named ACLs, mask inference, and the default-ACL path.

### Security

All hardening below was driven by a dedicated threat model and penetration test. Items marked **(opt-in)** default to v1-compatible behavior and change nothing unless enabled.

* **Hook timeout escalation:** SIGTERM, then 5s grace, then SIGKILL with a bounded reap. A hook that traps SIGTERM can no longer hang the agent indefinitely. First-signal behavior and the timeout exit code (3) match v1.
* **`scripts.log` is size-rotated** (64 MiB x 8 files) instead of appended without bound.
* **ANSI escape sequences are stripped** from `scripts.log` and the diagnostics shipped to CodeDeploy.
* **Process-group kill guard:** the agent refuses to signal PID 0 (its own group) or a PID greater than `i32::MAX`.
* **Full SELinux label applied:** `restorecon -vF` (not `-v`) so the AppSpec `user:` and `range:` fields actually reach the file label.
* **`kill_agent_max_wait_time_seconds` is clamped to 24 hours**, bounding a misconfiguration while preserving the 2-hour default.
* **Permission glob matching uses a linear-time matcher** (`globset`) instead of the hand-rolled matcher, preventing denial-of-service on adversarial AppSpec patterns (CWE-1333).
* **Hook script location is confined to the deployment archive**, and path-traversal containment checks fail closed on an unresolvable path.
* Secrets are redacted from credential debug output, and GitHub tokens are redacted from logs.
* The master process runs with umask `0027` (Unix) so agent files created without an explicit mode are not world-readable. The worker resets its umask to `0022` before extracting customer bundles, so archive entries with no stored Unix mode land at `0644` and stay readable by the service user.
* Created parent directories are `chmod`ed to their exact intended mode.
* **(opt-in)** `reject_symlinks_in_bundle` — reject bundles containing symlinks or hardlinks.
* **(opt-in)** `reject_path_traversal_in_bundle` — reject `..`/absolute archive entries (pre- and post-extraction) and `..`-escaping AppSpec `source`, `destination`, and `hooks.location` fields.
* **(opt-in)** `reject_unsafe_permissions_in_bundle` — reject SUID/SGID files in bundles and SUID/SGID AppSpec modes.
* **(opt-in)** `reject_unconfined_selinux_in_bundle` — reject SELinux types that disable mandatory access control (`unconfined_t`/`kernel_t`/`init_t`).
* **(opt-in)** `archive_max_extraction_size` — cap the declared uncompressed bundle size (archive-bomb guard; trusts archive headers). Also rejects truncated or malformed archives during header inspection.
* **(opt-in)** `reject_symlink_permission_targets` — reject a symlinked destination in an AppSpec `permissions:` block and apply chmod/chown/setfacl/SELinux context with no-follow, closing a TOCTOU privilege-escalation window (CWE-59 / CWE-367). Off by default because bundles that deliberately target a symlink deployed successfully on earlier versions. The write-side symlink race is closed unconditionally regardless of this flag.
* **(opt-in)** `ignore_ownership_in_bundle` — own files extracted from tar/tgz bundles as the extracting process instead of applying the archive header's uid/gid. Zip bundles are root-owned either way.
* **(opt-in)** `restrict_agent_dir_permissions` — root-only modes for the agent install root, per-deployment logs, and agent state files (see "Changed").
* **(opt-in)** `restrict_log_dir_permissions` — root+group-only modes for the agent log directory. Non-root log collectors (CloudWatch agent, fluentd, Datadog) lose access to the agent and updater logs when this is set.
* **(opt-in)** `restrict_hook_env_to_allowlist` — restrict lifecycle hooks to the documented deployment variables (`LIFECYCLE_EVENT`, `DEPLOYMENT_ID`, `APPLICATION_NAME`, `DEPLOYMENT_GROUP_NAME`, `DEPLOYMENT_GROUP_ID`, `BUNDLE_*`) plus a minimal shell environment (`PATH`/`HOME`/`USER`), instead of inheriting the full process environment.
* **(opt-in)** `strip_loader_env_in_hooks` — strip `LD_PRELOAD`/`LD_LIBRARY_PATH`/`LD_AUDIT` from the hook environment (Unix), including values supplied through an AppSpec `environment:` block.
* **(opt-in)** `disable_powershell_profile_in_hooks` — run `.ps1` hooks with `-NoProfile -NonInteractive` (Windows), preventing profile-script injection and interactive hangs in service context.

### Removed

* The v1 process-manager configuration keys (`children`, `wait_between_spawning_children`, `shared_dir`, `user`, `instance_service_*`, `codedeploy_test_profile`) are no longer recognized. They are ignored when present in an upgraded config, and each is logged once so operators can see it had no effect.
* Daemon-level privilege dropping (`user`/`group` self-demotion). Use a systemd `User=` override, or AppSpec `runas:` for hooks.
* The flock-based PID lock file (single-instance is now enforced by the service manager and PID liveness).
* The dynamic plugin system. CodeDeploy is the only plugin and is statically dispatched.
* The `ssl_verify_peer` configuration key. TLS verification is always on and cannot be disabled.
* On-premises registration is not part of the agent binary. Continue to use the existing external registration tooling. The agent reads the resulting on-premises config.

### Compatibility notes

* Existing configuration files load unchanged, including symbol-style keys (`:key: value`) in both the agent config and the on-premises credentials file. Unknown and retired keys are ignored rather than fatal, and are logged once.
* On-disk layout, instruction/cleanup/tracking file names, deployment-archive structure, ETag verification, deployment-spec PKCS7 signature verification, and AppSpec semantics (versions, file/permission/ACL/SELinux/mode handling, hook ordering and rollback mapping, error codes) all match agent v1.
* Local deployment IDs use the form `local-<pid>` instead of v1's `d-XXXXXXXXX-local`. Scripts keying on the deployment-root folder name for local deploys are affected.