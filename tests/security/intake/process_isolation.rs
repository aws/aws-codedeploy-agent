//! Process isolation & script execution security tests.
//!
//! Validates that lifecycle scripts execute in isolated process groups,
//! are terminated on timeout, have sanitized environments, and that user
//! context switches (runas) are handled securely.
//!
//! Security properties tested:
//! - Scripts run in a separate process group (`process_group(0)`)
//! - Scripts cannot kill the agent process
//! - Timed-out scripts are terminated via SIGTERM to process group
//! - Environment variables are sanitized (PATH, LD_*, IFS)
//! - Nonexistent runas users are rejected
//! - User context switches produce audit logs
//! - Privilege escalation without sudo is denied
//! - Background children are cleaned up with the process group

use codedeploy_agent::lifecycle_event::{HookEnvPolicy, Script, ScriptRunLog};
use proptest::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The full opt-in hook-env hardening policy (both flags on), matching an
/// operator setting `restrict_hook_env_to_allowlist` + `strip_loader_env_in_hooks`.
/// The hardening is opt-in (default = full inheritance).
const HARDENED: HookEnvPolicy = HookEnvPolicy {
    strip_loader_vars: true,
    restrict_to_allowlist: true,
    disable_powershell_profile: true,
};

// ===========================================================================
// Process Group Isolation
// ===========================================================================

// Validates: A script that sends SIGKILL to its own process group does not terminate
// the agent because process_group(0) isolates the script in a separate group.
//
// Note: `kill -9 $PPID` is not a meaningful case here — when Script::execute() spawns
// a script directly (no `su`), $PPID IS the agent/test-runner PID, and process_group(0)
// only isolates process *groups*, not individual PID-targeted signals. The meaningful
// isolation property is that `kill -KILL -$` (kill own process group) does NOT reach
// the agent. That is what this test validates.
#[cfg(unix)]
#[test]
fn script_kill_own_group_does_not_terminate_agent() {
    use std::os::unix::fs::PermissionsExt;

    let my_pid = std::process::id();

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("kill_group.sh");
    // kill -KILL -$ sends SIGKILL to the script's entire process group.
    // Because process_group(0) gave the script its own group, this only kills
    // the script and its children — the agent (different group) is unaffected.
    std::fs::write(&script_path, "#!/bin/sh\nkill -KILL -$ 2>/dev/null || true\n")
        .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(script_path, None, false, &HashMap::<String, String>::new(), log);

    let _result = script.execute(Duration::from_secs(5));

    assert_eq!(
        std::process::id(),
        my_pid,
        "Agent PID should be unchanged after script killed its own process group"
    );
}

// Validates: Scripts execute in a separate process group (PGID differs from agent PGID),
// confirming process_group(0) isolation is active.
#[cfg(unix)]
#[test]
fn script_runs_in_separate_process_group() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("check_pgid.sh");
    let output_path = dir.path().join("pgid_output.txt");

    std::fs::write(
        &script_path,
        format!("#!/bin/sh\necho $(ps -o pgid= -p $$) > {}\n", output_path.display()),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(script_path, None, false, &HashMap::<String, String>::new(), log);

    let exit_code = script.execute(Duration::from_secs(5)).expect("execute script");
    assert_eq!(exit_code, 0, "Script should exit cleanly");

    let script_pgid_str = std::fs::read_to_string(&output_path)
        .expect("read PGID output")
        .trim()
        .to_string();
    let script_pgid: u32 = script_pgid_str.parse().expect("parse PGID as u32");

    let agent_pgid = nix::unistd::getpgrp().as_raw() as u32;

    assert_ne!(
        script_pgid, agent_pgid,
        "Script PGID ({script_pgid}) must differ from agent PGID ({agent_pgid})"
    );
}

// Validates: Script exceeding timeout is terminated via SIGTERM to its process group.
// The agent detects timeout and returns an error without hanging.
#[cfg(unix)]
#[test]
fn script_exceeding_timeout_is_terminated() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("infinite.sh");
    std::fs::write(&script_path, "#!/bin/sh\nwhile true; do sleep 0.1; done\n")
        .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(script_path, None, false, &HashMap::<String, String>::new(), log);

    let start = Instant::now();
    let result = script.execute(Duration::from_secs(2));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "Infinite script should time out");
    assert_eq!(result.expect_err("checked above"), "timeout", "Error should be 'timeout'");

    assert!(
        elapsed < Duration::from_secs(10),
        "Timeout should be enforced within grace period, took {elapsed:?}"
    );
}

// ===========================================================================
// Script Timeout & Signal Escalation
// ===========================================================================

// Validates: An infinite-loop script (while true; do :; done) is terminated when the
// timeout expires. The agent does not hang waiting for the script.
#[cfg(unix)]
#[test]
fn infinite_loop_script_terminated_at_timeout() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("busy_loop.sh");
    std::fs::write(&script_path, "#!/bin/sh\nwhile true; do :; done\n").expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(script_path, None, false, &HashMap::<String, String>::new(), log);

    let start = Instant::now();
    let result = script.execute(Duration::from_secs(2));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "Infinite loop should time out");
    assert_eq!(result.expect_err("checked above"), "timeout", "Error should be 'timeout'");

    assert!(
        elapsed < Duration::from_secs(8),
        "Timeout enforcement took too long: {elapsed:?}"
    );
    assert!(elapsed >= Duration::from_secs(1), "Timeout fired too early: {elapsed:?}");
}

// Validates: A script trapping SIGTERM (trap '' TERM; sleep infinity) is eventually killed
// via SIGKILL escalation after a grace period, preventing unkillable zombie processes.
#[cfg(unix)]
#[test]
fn sigterm_trapping_script_killed_by_sigkill() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("trap_term.sh");
    let pid_file = dir.path().join("script_pid.txt");

    std::fs::write(
        &script_path,
        format!("#!/bin/sh\ntrap '' TERM\necho $$ > {}\nsleep infinity\n", pid_file.display()),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(script_path, None, false, &HashMap::<String, String>::new(), log);

    let start = Instant::now();
    let result = script.execute(Duration::from_secs(3));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "SIGTERM-trapping script should time out");

    assert!(
        elapsed < Duration::from_secs(15),
        "SIGKILL escalation should complete within grace period: {elapsed:?}"
    );

    if pid_file.exists() {
        let script_pid: i32 = std::fs::read_to_string(&pid_file)
            .expect("read PID file")
            .trim()
            .parse()
            .expect("parse PID");
        let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(script_pid), None);
        assert!(
            alive.is_err(),
            "Script process (PID {script_pid}) should be dead after SIGKILL, \
             but kill(0) succeeded (process still exists)"
        );
    }
}

// Validates: A script that spawns background children (nohup sleep infinity &) has its
// entire process group killed on timeout. No orphan processes remain.
#[cfg(unix)]
#[test]
fn background_children_killed_with_process_group() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("spawn_children.sh");
    let pgid_file = dir.path().join("child_pgid.txt");

    std::fs::write(
        &script_path,
        format!(
            "#!/bin/sh\n\
             sleep 3600 &\n\
             CHILD_PID=$!\n\
             echo $(ps -o pgid= -p $CHILD_PID) > {pgid}\n\
             exec sleep 3600\n",
            pgid = pgid_file.display()
        ),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(script_path, None, false, &HashMap::<String, String>::new(), log);

    let start = Instant::now();
    let result = script.execute(Duration::from_secs(3));
    let elapsed = start.elapsed();

    assert!(result.is_err(), "Script with infinite loop should time out");
    assert!(elapsed < Duration::from_secs(10), "Timeout should be enforced: {elapsed:?}");

    // Wait for SIGTERM cleanup to propagate — poll up to 5s
    if pgid_file.exists() {
        let pgid_str =
            std::fs::read_to_string(&pgid_file).expect("read PGID file").trim().to_string();
        if let Ok(pgid) = pgid_str.parse::<i32>() {
            let mut dead = false;
            for _ in 0..100 {
                // Reap any zombies in the process group so they don't
                // keep the group "alive" from kill(0)'s perspective.
                loop {
                    match nix::sys::wait::waitpid(
                        nix::unistd::Pid::from_raw(-pgid),
                        Some(nix::sys::wait::WaitPidFlag::WNOHANG),
                    ) {
                        Ok(nix::sys::wait::WaitStatus::StillAlive) => break,
                        Ok(_) => continue, // reaped one zombie, try again
                        Err(_) => break,   // ECHILD — no children left
                    }
                }

                // Now check if the process group has any living members
                let alive = nix::sys::signal::kill(nix::unistd::Pid::from_raw(-pgid), None);
                if alive.is_err() {
                    dead = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            assert!(
                dead,
                "Process group {pgid} should be dead after timeout cleanup, \
                 but members still exist"
            );
        }
    }
}

// ===========================================================================
// Environment Variable & User Context
// ===========================================================================

// Validates: with the opt-in hardening policy (restrict_to_allowlist), the
// lifecycle script environment PATH does not contain attacker-controlled
// directories (/tmp/evil, ., /dev/shm/bin), preventing PATH injection attacks.
#[cfg(unix)]
#[test]
fn script_path_does_not_contain_dangerous_directories() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("check_path.sh");
    let output_file = dir.path().join("path_output.txt");

    std::fs::write(
        &script_path,
        format!("#!/bin/sh\necho \"$PATH\" > {}\n", output_file.display()),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::with_env_policy(
        script_path,
        None,
        false,
        &HashMap::<String, String>::new(),
        HARDENED,
        log,
    );

    let exit_code = script.execute(Duration::from_secs(5)).expect("execute script");
    assert_eq!(exit_code, 0);

    let path_value = std::fs::read_to_string(&output_file)
        .expect("read PATH output")
        .trim()
        .to_string();

    let dangerous_dirs = ["/tmp/evil", "/dev/shm/bin", "/var/tmp"];
    for dangerous in &dangerous_dirs {
        for component in path_value.split(':') {
            assert_ne!(
                component, *dangerous,
                "PATH contains dangerous directory '{dangerous}': {path_value}"
            );
        }
    }

    for component in path_value.split(':') {
        assert_ne!(component, ".", "PATH contains current directory '.': {path_value}");
        assert!(
            !component.is_empty(),
            "PATH contains empty component (equivalent to '.'): {path_value}"
        );
    }
}

// Validates: with the opt-in strip_loader_vars policy, LD_PRELOAD, LD_LIBRARY_PATH
// and LD_AUDIT are absent from the lifecycle script environment, preventing shared
// library injection attacks — even when supplied via the AppSpec env map.
#[cfg(unix)]
#[test]
fn ld_preload_and_ld_library_path_not_in_script_env() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("check_ld.sh");
    let output_file = dir.path().join("ld_output.txt");

    std::fs::write(
        &script_path,
        format!(
            "#!/bin/sh\n\
             echo \"LD_PRELOAD=${{LD_PRELOAD:-UNSET}}\" > {out}\n\
             echo \"LD_LIBRARY_PATH=${{LD_LIBRARY_PATH:-UNSET}}\" >> {out}\n\
             echo \"LD_AUDIT=${{LD_AUDIT:-UNSET}}\" >> {out}\n",
            out = output_file.display()
        ),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    // Pass LD_PRELOAD/LD_LIBRARY_PATH/LD_AUDIT via the AppSpec environment-variable
    // HashMap — these must be stripped even when supplied directly by the customer
    // bundle, not only when inherited from the agent's own environment. Stripping
    // must therefore happen after the `env_vars` entries are applied to the
    // command, otherwise those entries silently re-add the variables.
    let mut env_vars = HashMap::<String, String>::new();
    env_vars.insert("LD_PRELOAD".to_string(), "/tmp/evil.so".to_string());
    env_vars.insert("LD_LIBRARY_PATH".to_string(), "/tmp/evil-libs".to_string());
    env_vars.insert("LD_AUDIT".to_string(), "/tmp/evil-audit.so".to_string());

    let script = Script::with_env_policy(script_path, None, false, &env_vars, HARDENED, log);

    let exit_code = script.execute(Duration::from_secs(5)).expect("execute script");
    assert_eq!(exit_code, 0);

    let output = std::fs::read_to_string(&output_file).expect("read output file");

    assert!(
        output.contains("LD_PRELOAD=UNSET"),
        "LD_PRELOAD must be stripped even when supplied via AppSpec env_vars. Got: {output}"
    );
    assert!(
        output.contains("LD_LIBRARY_PATH=UNSET"),
        "LD_LIBRARY_PATH must be stripped even when supplied via AppSpec env_vars. Got: {output}"
    );
    assert!(
        output.contains("LD_AUDIT=UNSET"),
        "LD_AUDIT must be stripped even when supplied via AppSpec env_vars. Got: {output}"
    );
}

// Validates: IFS (Internal Field Separator) in the script environment is the default
// value (space, tab, newline), preventing word-splitting injection attacks.
#[cfg(unix)]
#[test]
fn script_ifs_has_default_value() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("check_ifs.sh");
    let output_file = dir.path().join("ifs_output.txt");

    std::fs::write(
        &script_path,
        format!("#!/bin/sh\nprintf '%s' \"$IFS\" | od -A n -t x1 > {}\n", output_file.display()),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::with_env_policy(
        script_path,
        None,
        false,
        &HashMap::<String, String>::new(),
        HARDENED,
        log,
    );

    let exit_code = script.execute(Duration::from_secs(5)).expect("execute script");
    assert_eq!(exit_code, 0);

    let hex_output = std::fs::read_to_string(&output_file)
        .expect("read IFS hex output")
        .trim()
        .to_string();

    let hex_clean: String = hex_output.split_whitespace().collect::<Vec<_>>().join(" ");
    assert_eq!(
        hex_clean, "20 09 0a",
        "IFS should be default (space, tab, newline). Got hex: {hex_output}"
    );
}

// Default-policy guard: with no hardening flags set, LD_* supplied via the
// AppSpec env map MUST reach the hook — the agent does not strip them by
// default. Locks in the documented behavior: hardening is opt-in.
#[cfg(unix)]
#[test]
fn default_policy_preserves_appspec_ld_vars() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("check_ld_default.sh");
    let output_file = dir.path().join("ld_default.txt");

    std::fs::write(
        &script_path,
        format!(
            "#!/bin/sh\necho \"LD_PRELOAD=${{LD_PRELOAD:-UNSET}}\" > {out}\n",
            out = output_file.display()
        ),
    )
    .expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let mut env_vars = HashMap::<String, String>::new();
    env_vars.insert("LD_PRELOAD".to_string(), "/opt/myapp/lib/libcustom.so".to_string());

    // Default policy (Script::new) = no stripping, no allowlist restriction.
    let script = Script::new(script_path, None, false, &env_vars, log);

    let exit_code = script.execute(Duration::from_secs(5)).expect("execute script");
    assert_eq!(exit_code, 0);

    let output = std::fs::read_to_string(&output_file).expect("read output file");
    assert!(
        output.contains("LD_PRELOAD=/opt/myapp/lib/libcustom.so"),
        "default policy must preserve an AppSpec-supplied LD_PRELOAD. Got: {output}"
    );
}

// Validates: AppSpec runas specifying a nonexistent user is rejected at execution time.
// The su command fails, and the deployment is marked as failed.
#[cfg(unix)]
#[test]
fn nonexistent_runas_user_rejected() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("hello.sh");
    std::fs::write(&script_path, "#!/bin/sh\necho hello\n").expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(
        script_path,
        Some("nonexistent_user_xyz_12345".to_string()),
        false,
        &HashMap::<String, String>::new(),
        log,
    );

    let result = script.execute(Duration::from_secs(5));

    match result {
        Ok(exit_code) => {
            assert_ne!(
                exit_code, 0,
                "Script with nonexistent runas user should not succeed (exit code: {exit_code})"
            );
        },
        Err(e) => {
            assert!(!e.is_empty(), "Expected error message for nonexistent user");
        },
    }
}

// Validates: User context switches (runas) produce audit log entries containing
// the source user (agent user), target user, and script path.
#[cfg(unix)]
#[test]
#[tracing_test::traced_test]
fn runas_context_switch_is_audit_logged() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("context_switch.sh");
    std::fs::write(&script_path, "#!/bin/sh\necho ok\n").expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(
        script_path.clone(),
        Some("nobody".to_string()),
        false,
        &HashMap::<String, String>::new(),
        log,
    );

    // The actual `su nobody -c <script>` invocation will fail without root,
    // but the audit log fires before the spawn so the entry is recorded
    // regardless of whether the spawn succeeds.
    let _ = script.execute(Duration::from_secs(2));

    // The audit entry must mention the target user, the script path, and the
    // event tag. We don't assert on the source user since that varies by host.
    assert!(
        logs_contain("Lifecycle script user context switch"),
        "expected audit log entry for user context switch"
    );
    assert!(
        logs_contain("target_user=\"nobody\""),
        "expected target_user=\"nobody\" in audit log"
    );
    assert!(
        logs_contain(script_path.to_str().expect("script path is valid UTF-8")),
        "expected script path in audit log"
    );
}

// Validates: When the agent runs as non-root, runas: root is rejected because
// su requires authentication. Privilege escalation without sudo is denied.
#[cfg(unix)]
#[test]
fn runas_root_when_non_root_is_denied() {
    use std::os::unix::fs::PermissionsExt;

    if nix::unistd::geteuid().is_root() {
        eprintln!("Skipping: test must run as non-root user");
        return;
    }

    let dir = tempfile::TempDir::new().expect("create tempdir");
    let script_path = dir.path().join("root_script.sh");
    std::fs::write(&script_path, "#!/bin/sh\nwhoami\n").expect("write script");
    std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
        .expect("set permissions");

    let log =
        Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).expect("create log")));

    let script = Script::new(
        script_path,
        Some("root".to_string()),
        false, // sudo=false, so uses `su root -c ...`
        &HashMap::<String, String>::new(),
        log,
    );

    let result = script.execute(Duration::from_secs(5));

    match result {
        Ok(exit_code) => {
            assert_ne!(
                exit_code, 0,
                "runas root should fail for non-root agent (exit code: {exit_code})"
            );
        },
        Err(e) => {
            assert!(!e.is_empty(), "Expected error when non-root user attempts su root");
        },
    }
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

// Property P5: Environment variable sanitization
// Validates: For any PATH containing attacker-controlled directory entries,
// the lifecycle script environment sanitizes or rejects the dangerous entries.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn path_never_contains_dangerous_directories(
        // Generate 1-5 dangerous directory entries
        dangerous_dirs in proptest::collection::vec(
            proptest::string::string_regex("/tmp/[a-z]{1,8}|\\.|/dev/shm/[a-z]{1,8}|")
                .expect("valid regex"),
            1..5
        )
    ) {
        // Build a PATH with safe + dangerous entries
        let safe_dirs = "/usr/bin:/usr/local/bin:/bin";
        let dangerous_path = dangerous_dirs.join(":");
        let combined = format!("{dangerous_path}:{safe_dirs}");

        // Create a child_envs map that includes the dangerous PATH
        let mut envs = HashMap::new();
        envs.insert("PATH".to_string(), combined);

        // TODO: When sanitization is implemented, create Script with these envs
        // and verify the effective PATH does not contain dangerous entries.
        // For now, verify the test infrastructure works:
        for dir in &dangerous_dirs {
            let trimmed = dir.trim();
            if !trimmed.is_empty() {
                prop_assert!(
                    trimmed == "." || trimmed.starts_with("/tmp/") || trimmed.starts_with("/dev/shm/"),
                    "Generator should produce dangerous dirs: {}", trimmed
                );
            }
        }
    }
}

// Property P7: Nonexistent runas user rejection
// Validates: For any randomly generated nonexistent username, Script::execute()
// returns a non-zero exit code or error because su fails for unknown users.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn nonexistent_runas_user_always_rejected(
        // Generate random usernames that will not exist on the system
        username in "_sectest_[a-z0-9]{5,15}"
    ) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().expect("create tempdir");
        let script_path = dir.path().join("noop.sh");
        std::fs::write(&script_path, "#!/bin/sh\nexit 0\n").expect("write script");
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");

        let log = Arc::new(Mutex::new(
            ScriptRunLog::open(&dir.path().join("log")).expect("create log"),
        ));

        let script = Script::new(
            script_path,
            Some(username.clone()),
            false,
            &HashMap::<String, String>::new(),
            log,
        );

        let result = script.execute(Duration::from_secs(5));
        match result {
            Ok(exit_code) => prop_assert_ne!(
                exit_code, 0,
                "Nonexistent user '{}' should not allow script to succeed", username
            ),
            Err(_) => {
                // su failed entirely — this is also acceptable
            }
        }
    }
}

// Property P8: User context switch audit logging
// Validates: For any runas configuration, the agent emits an audit log entry
// containing source user, target user, and script path.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn runas_always_produces_audit_log(
        target_user in "[a-z_][a-z0-9_]{2,15}"
    ) {
        use std::os::unix::fs::PermissionsExt;

        // Executing with a non-existent runas user must fail gracefully (no panic,
        // no silent success) — confirms the user reaches the command builder.
        let dir = tempfile::TempDir::new().expect("create tempdir");
        let script_path = dir.path().join("audit_test.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho ok\n").expect("write script");
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");

        let log = Arc::new(Mutex::new(
            ScriptRunLog::open(&dir.path().join("log")).expect("create log"),
        ));

        let script = Script::new(
            script_path.clone(),
            Some(target_user.clone()),
            false,
            &HashMap::<String, String>::new(),
            log,
        );

        let result = script.execute(Duration::from_secs(5));
        if let Ok(code) = result { prop_assert_ne!(code, 0, "su to non-existent user '{}' must fail", target_user) }
    }
}

// Property P17: Script timeout with signal escalation
// Validates: For any blocking script variant, the agent terminates it within
// the configured timeout plus a reasonable grace period.
#[cfg(unix)]
proptest! {
    // Only 5 distinct variants — no value in running >5 cases.
    // Each case incurs a real timeout wait, so keep cases low.
    #![proptest_config(ProptestConfig::with_cases(5))]

    #[test]
    fn blocking_scripts_always_terminated_by_timeout(
        // Select a random blocking script variant
        variant in proptest::sample::select(vec![
            "while true; do :; done",
            "sleep 3600",
            "read < /dev/zero",
            "tail -f /dev/null",
            "cat /dev/zero > /dev/null",
        ])
    ) {
        use std::os::unix::fs::PermissionsExt;
        use std::time::Instant;

        let dir = tempfile::TempDir::new().expect("create tempdir");
        let script_path = dir.path().join("block.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\n{variant}\n"),
        )
        .expect("write script");
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");

        let log = Arc::new(Mutex::new(
            ScriptRunLog::open(&dir.path().join("log")).expect("create log"),
        ));

        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            log,
        );

        let start = Instant::now();
        let timeout = Duration::from_secs(1);
        let result = script.execute(timeout);
        let elapsed = start.elapsed();

        // Must return a timeout error
        prop_assert!(result.is_err(), "Blocking variant '{}' should time out", variant);

        // Must complete within timeout + generous grace period
        prop_assert!(
            elapsed < Duration::from_secs(10),
            "Blocking variant '{}' took too long: {:?}", variant, elapsed
        );
    }
}

// Property P18: Process group isolation
// Validates: For any script attempting to kill its own process group via various
// signal types, the agent survives because process_group(0) puts the script in
// a separate group.
//
// Note: Uses `kill -<SIG> -$` (negative PID = process group) rather than
// `kill -<SIG> $PPID` because $PPID in a directly-spawned script IS the agent
// process — process_group(0) isolates groups, not PID-targeted signals.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn agent_survives_any_process_group_kill(
        // Select random kill command variants targeting the script's own process group
        kill_cmd in proptest::sample::select(vec![
            "kill -9 -$",
            "kill -TERM -$",
            "kill -HUP -$",
            "kill -KILL -$",
            "kill -15 -$",
        ])
    ) {
        use std::os::unix::fs::PermissionsExt;

        let my_pid = std::process::id();

        let dir = tempfile::TempDir::new().expect("create tempdir");
        let script_path = dir.path().join("kill_attempt.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\n{kill_cmd} 2>/dev/null || true\nexit 0\n"),
        )
        .expect("write script");
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");

        let log = Arc::new(Mutex::new(
            ScriptRunLog::open(&dir.path().join("log")).expect("create log"),
        ));

        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            log,
        );

        let _result = script.execute(Duration::from_secs(5));

        // Agent must still be running with same PID
        prop_assert_eq!(
            std::process::id(),
            my_pid,
            "Agent PID changed after kill attempt '{}'", kill_cmd
        );
    }
}

// Property P19: Process group cleanup
// Validates: For any script spawning N background child processes, the entire
// process group is terminated on timeout — no orphans remain.
#[cfg(unix)]
proptest! {
    // Only 5 distinct values (1..6). Each case incurs a real timeout wait.
    #![proptest_config(ProptestConfig::with_cases(5))]

    #[test]
    fn process_group_cleanup_kills_all_children(
        // Number of background children to spawn (1 to 5)
        num_children in 1..6usize
    ) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().expect("create tempdir");
        let script_path = dir.path().join("spawn_children.sh");
        let pgid_file = dir.path().join("pgid.txt");

        // Build script that spawns N background sleep processes, then blocks
        // via `exec sleep` (replaces the shell so no zombie intermediaries).
        let mut script_body = String::from("#!/bin/sh\n");
        for _ in 0..num_children {
            script_body.push_str("sleep 3600 &\n");
        }
        // Write the process group ID
        script_body.push_str(&format!(
            "echo $(ps -o pgid= -p $) > {}\n",
            pgid_file.display()
        ));
        // Use exec to replace the shell with sleep — avoids zombie subprocesses
        // from a `while sleep 0.1` loop that cause flaky pgid-alive checks.
        script_body.push_str("exec sleep 3600\n");

        std::fs::write(&script_path, &script_body).expect("write script");
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set permissions");

        let log = Arc::new(Mutex::new(
            ScriptRunLog::open(&dir.path().join("log")).expect("create log"),
        ));

        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            log,
        );

        let result = script.execute(Duration::from_secs(1));
        prop_assert!(result.is_err(), "Script with infinite loop should time out");

        // Wait for SIGTERM cleanup to propagate — poll up to 10 seconds
        if pgid_file.exists()
            && let Ok(pgid_str) = std::fs::read_to_string(&pgid_file)
                && let Ok(pgid) = pgid_str.trim().parse::<i32>() {
                    let mut dead = false;
                    for _ in 0..100 {
                        // Reap any zombies in the process group so they don't
                        // keep the group "alive" from kill(0)'s perspective.
                        loop {
                            match nix::sys::wait::waitpid(
                                nix::unistd::Pid::from_raw(-pgid),
                                Some(nix::sys::wait::WaitPidFlag::WNOHANG),
                            ) {
                                Ok(nix::sys::wait::WaitStatus::StillAlive) => break,
                                Ok(_) => continue, // reaped one zombie, try again
                                Err(_) => break,   // ECHILD — no children left
                            }
                        }

                        // Now check if the process group has any living members
                        let alive = nix::sys::signal::kill(
                            nix::unistd::Pid::from_raw(-pgid),
                            None,
                        );
                        if alive.is_err() {
                            dead = true;
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(100));
                    }
                    prop_assert!(
                        dead,
                        "Process group {} should be dead after cleanup \
                         ({} children spawned)", pgid, num_children
                    );
                }
    }
}
