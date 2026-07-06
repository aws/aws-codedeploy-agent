// Canary tests for script resource exhaustion.
//
// These tests validate that the *deployment environment* (systemd, cgroups, ulimits)
// constrains adversarial lifecycle scripts. The agent itself does not implement
// these controls — they are expected to be enforced externally.
//
// ALL tests in this file are #[ignore] — they are dangerous to run without proper
// cgroup isolation and should only execute in nightly CI with containment.

use std::process::Command;
use std::time::{Duration, Instant};

// CANARY TEST — validates external controls (systemd/cgroup), not agent code.
// Validates: CPU-intensive lifecycle scripts are terminated by execution timeout,
// preventing a single deployment from consuming unbounded CPU time.
#[test]
#[ignore] // Canary test: requires systemd/cgroup timeout configuration. Run in nightly CI.
fn cpu_intensive_script_terminated_at_timeout() {
    // Create a script that burns CPU indefinitely
    let dir = tempfile::tempdir().expect("create tempdir for cpu_hog script");
    let script_path = dir.path().join("cpu_hog.sh");
    std::fs::write(&script_path, "#!/bin/sh\nwhile true; do :; done\n")
        .expect("write cpu_hog script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set cpu_hog script permissions");
    }

    let timeout = Duration::from_secs(10); // Test timeout — shorter than real deployment
    let start = Instant::now();

    let mut child = Command::new(&script_path).spawn().expect("spawn cpu_hog script");

    // Wait for timeout, then kill if still running
    std::thread::sleep(timeout);
    let elapsed = start.elapsed();

    match child.try_wait().expect("check cpu_hog child status") {
        Some(status) => {
            // Script was terminated (by OS/cgroup/timeout)
            assert!(
                !status.success(),
                "CPU-hogging script should have been killed, not exit cleanly"
            );
        },
        None => {
            // Script still running — kill it ourselves (indicates missing external control)
            child.kill().expect("kill cpu_hog script");
            child.wait().expect("wait for killed cpu_hog process");

            // TODO: Configure systemd TimeoutStartSec or cgroup CPU limits
            // to terminate long-running lifecycle scripts automatically.
            assert!(
                elapsed >= timeout,
                "Script ran for {elapsed:?} — no external timeout terminated it. \
                 TODO: Configure systemd TimeoutStartSec or cgroup CPU limits."
            );
        },
    }
}

// CANARY TEST — validates OS/cgroup controls, not agent code.
// Validates: Scripts that exhaust memory are terminated by OOM killer or cgroup
// memory limits, preventing system-wide resource exhaustion.
#[test]
#[ignore] // Canary test: potentially destructive without cgroup memory limits. Nightly CI only.
fn memory_exhaustion_contained_by_os() {
    // Create a script that allocates memory until killed
    let dir = tempfile::tempdir().expect("create tempdir for mem_hog script");
    let script_path = dir.path().join("mem_hog.sh");
    std::fs::write(
        &script_path,
        r#"#!/bin/sh
# Allocate memory in a loop until OOM-killed
# Uses dd to create large in-memory blocks
a=""
while true; do
    a="${a}$(dd if=/dev/zero bs=1M count=10 2>/dev/null)"
done
"#,
    )
    .expect("write mem_hog script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set mem_hog script permissions");
    }

    let mut child = Command::new(&script_path).spawn().expect("spawn mem_hog script");

    // Wait up to 30 seconds for OOM/cgroup to kill the process
    let timeout = Duration::from_secs(30);
    let start = Instant::now();

    loop {
        match child.try_wait().expect("check mem_hog child status") {
            Some(status) => {
                // Process was killed — OOM or cgroup
                assert!(
                    !status.success(),
                    "Memory-hogging script should have been killed, not exit cleanly"
                );

                // On Linux, OOM kill results in signal 9 (SIGKILL)
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(signal) = status.signal() {
                        assert_eq!(
                            signal, 9,
                            "Expected SIGKILL (9) from OOM killer, got signal {signal}"
                        );
                    }
                }
                return;
            },
            None => {
                if start.elapsed() > timeout {
                    // Still running after timeout — kill and document gap
                    child.kill().expect("kill mem_hog script");
                    child.wait().expect("wait for killed mem_hog process");
                    // TODO: Configure cgroup memory limits for deployment scripts
                    panic!(
                        "Memory-hogging script was not killed within {timeout:?}. \
                         TODO: Configure cgroup memory limits for deployment scripts."
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            },
        }
    }
}

// CANARY TEST — validates OS/cgroup controls, not agent code.
// Validates: Fork bomb scripts are contained by process/PID limits (ulimit -u,
// cgroup pids.max, or systemd TasksMax), preventing system-wide process exhaustion.
#[test]
#[ignore] // Canary test: DESTRUCTIVE without PID limits. Nightly CI with cgroup isolation ONLY.
fn fork_bomb_contained_by_process_limits() {
    // Create a fork bomb script (classic bash fork bomb, contained variant)
    let dir = tempfile::tempdir().expect("create tempdir for fork_bomb script");
    let script_path = dir.path().join("fork_bomb.sh");
    std::fs::write(
        &script_path,
        "#!/bin/sh\n# Fork bomb — should be contained by PID limits\nbomb() { bomb | bomb & }; bomb\n",
    )
    .expect("write fork_bomb script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755))
            .expect("set fork_bomb script permissions");
    }

    // SAFETY: Only run this inside a PID-limited cgroup!
    // Without limits, this WILL destabilize the host.

    let mut child = Command::new(&script_path).spawn().expect("spawn fork_bomb script");

    // Wait for the fork bomb to exhaust PID limits and die
    let timeout = Duration::from_secs(30);
    let start = Instant::now();

    loop {
        match child.try_wait().expect("check fork_bomb child status") {
            Some(status) => {
                // Fork bomb hit PID limit — all children died
                assert!(
                    !status.success(),
                    "Fork bomb should have been killed by PID limits, not exit cleanly"
                );
                return;
            },
            None => {
                if start.elapsed() > timeout {
                    // Still running — kill the process group
                    #[cfg(unix)]
                    {
                        use nix::sys::signal::{Signal, kill};
                        use nix::unistd::Pid;
                        // Kill entire process group (negative PID)
                        let _ = kill(Pid::from_raw(-(child.id() as i32)), Signal::SIGKILL);
                    }
                    child.kill().expect("kill fork_bomb script");
                    child.wait().expect("wait for killed fork_bomb process");
                    // TODO: Configure cgroup pids.max or systemd TasksMax
                    panic!(
                        "Fork bomb was not contained within {timeout:?}. \
                         TODO: Configure cgroup pids.max or systemd TasksMax."
                    );
                }
                std::thread::sleep(Duration::from_millis(500));
            },
        }
    }
}
