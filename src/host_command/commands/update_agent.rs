//! `UpdateDeploymentAgent` command — agent self-update.
//!
//! Downloads the `install` script from the `aws-codedeploy-{region}` S3 bucket
//! and runs it. The script is the single source of truth for update logic
//! (package-manager + arch detection, package download and install); the
//! package's post-install scripts restart the agent.

use crate::aws_clients::S3Client;
use crate::logging::updater_log_path;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::process::{Command, Output};
use tempfile::NamedTempFile;
use tracing::{error, info, warn};

/// S3 key of the install/update script in the regional agent bucket. The 2.x
/// release layout publishes under `latestv2/`; the legacy `latest/` prefix is
/// frozen on the 1.x installer, which would fail or downgrade the agent here.
const INSTALL_SCRIPT_KEY: &str = "latestv2/install";

/// Interpreter for the install script — run via the shell rather than relying
/// on the downloaded file's execute bit or shebang.
const SHELL: &str = "/bin/sh";

/// Package-type arg: `auto` tells the script to detect the package manager.
const PACKAGE_TYPE_ARG: &str = "auto";

pub struct UpdateAgentCommand {
    region: String,
    s3_client: Option<S3Client>,
    /// Mode policy for the updater log, from `restrict_log_dir_permissions`.
    restrict_log_permissions: bool,
}

impl std::fmt::Debug for UpdateAgentCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateAgentCommand")
            .field("region", &self.region)
            .field("s3_client", &self.s3_client.is_some())
            .field("restrict_log_permissions", &self.restrict_log_permissions)
            .finish()
    }
}

impl UpdateAgentCommand {
    #[must_use]
    pub fn new(region: String, s3_client: Option<S3Client>) -> Self {
        Self { region, s3_client, restrict_log_permissions: false }
    }

    /// Set the mode policy for the updater log, from
    /// `restrict_log_dir_permissions`. Defaults to `false` (0644).
    #[must_use]
    pub fn with_restrict_log_permissions(mut self, restrict: bool) -> Self {
        self.restrict_log_permissions = restrict;
        self
    }

    /// Execute the `UpdateDeploymentAgent` command.
    ///
    /// Downloads the `install` script from the regional agent bucket and runs
    /// it with the `auto` package type. The script detects the package manager
    /// and architecture, downloads the latest package, and installs it; the
    /// package's post-install scripts handle restarting the agent.
    ///
    /// # Errors
    /// Returns an error if the S3 client is unconfigured, the script download
    /// fails, or the script exits non-zero.
    // GRCOV_STOP_COVERAGE — execute() orchestrates S3 downloads and a subprocess
    pub fn execute(&self) -> io::Result<Vec<String>> {
        info!("UpdateDeploymentAgent command received");

        let s3 = self
            .s3_client
            .as_ref()
            .ok_or_else(|| io::Error::other("S3 client not configured for agent update"))?;

        let bucket = s3_bucket_name(&self.region);

        // Download to a temp file (created 0600 by stream_to_file, auto-cleaned on drop).
        let tmp_file = NamedTempFile::new()?;
        let tmp_path = tmp_file.path();
        s3.download_to_file(&bucket, INSTALL_SCRIPT_KEY, None, tmp_path)?;
        info!(path = %tmp_path.display(), key = INSTALL_SCRIPT_KEY, "Downloaded agent install script");

        run_install_script(tmp_path, &self.region, self.restrict_log_permissions)?;

        info!("Agent install script completed — post-install scripts will restart the agent");
        Ok(vec!["Update installed successfully".to_string()])
    }
    // GRCOV_BEGIN_COVERAGE
}

/// Construct the S3 bucket name for agent packages.
fn s3_bucket_name(region: &str) -> String {
    format!("aws-codedeploy-{region}")
}

/// Run the install script, capture its output to the updater log, and map its
/// exit status to a `Result`.
// GRCOV_STOP_COVERAGE — subprocess + fixed-path log; logic tested via
// execute_script / interpret_status below.
fn run_install_script(script_path: &Path, region: &str, restrict_log: bool) -> io::Result<()> {
    let output = execute_script(script_path, region)?;

    // Best-effort: a logging failure must not mask the install result.
    if let Err(e) =
        append_updater_log(&updater_log_path(), SHELL, script_path, &output, restrict_log)
    {
        warn!(error = %e, path = %updater_log_path().display(), "Failed to write updater log");
    }

    interpret_status(&output)
}
// GRCOV_BEGIN_COVERAGE

/// Run the install script through the shell with the `auto` package type.
///
/// `AWS_REGION` is exported so the script reuses the region the agent already
/// resolved instead of re-querying IMDS.
fn execute_script(script_path: &Path, region: &str) -> io::Result<Output> {
    if !script_path.is_file() {
        return Err(io::Error::other(format!(
            "Install script not found at {}",
            script_path.display()
        )));
    }

    info!(
        shell = SHELL,
        script = %script_path.display(),
        package_type = PACKAGE_TYPE_ARG,
        "Running agent install script"
    );

    let mut cmd = Command::new(SHELL);
    cmd.arg(script_path).arg(PACKAGE_TYPE_ARG).env("AWS_REGION", region);
    strip_dynamic_loader_vars(&mut cmd);
    cmd.output()
}

/// Strip `LD_*` loader vars so they can't inject a shared library into the
/// root-run install script or its sub-processes. Mirrors `Script::execute_async`.
/// Call after any `.env()` mutation so the removal is unconditional.
fn strip_dynamic_loader_vars(cmd: &mut Command) {
    #[cfg(unix)]
    for var in ["LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT"] {
        cmd.env_remove(var);
    }
    #[cfg(not(unix))]
    let _ = cmd; // LD_* is unix-only
}

/// Translate the script's exit status into a `Result`, logging failures (exit
/// code + stderr) at `error` level so they surface in the agent log.
fn interpret_status(output: &Output) -> io::Result<()> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        error!(
            exit_code = output.status.code(),
            stderr = %stderr,
            stdout = %stdout,
            "Agent install script failed"
        );
        return Err(io::Error::other(format!(
            "Agent install script failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    info!(stdout = %stdout, "Agent install script succeeded");
    Ok(())
}

/// Append a timestamped record of an updater subprocess invocation to
/// `log_path`, creating the parent directory and file if needed.
///
/// On Linux the file is opened with `O_NOFOLLOW` so a pre-existing symlink at
/// the target path causes the open to fail rather than redirect the write
/// (defense-in-depth against symlink/TOCTOU attacks).
fn append_updater_log(
    log_path: &Path,
    cmd: &str,
    target_path: &Path,
    output: &Output,
    restrict_log: bool,
) -> io::Result<()> {
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?; // GRCOV_IGNORE_LINE
    }

    let mut opts = OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // Policy mode from `restrict_log_dir_permissions`: 0644 default
        // (world-readable, matching the agent log), 0640 hardened.
        opts.mode(if restrict_log { 0o640 } else { 0o644 })
            .custom_flags(nix::libc::O_NOFOLLOW);
    }
    // Windows: no O_NOFOLLOW equivalent. Check for symlink before opening as
    // defense-in-depth against symlink/TOCTOU attacks. This is weaker than the
    // Unix O_NOFOLLOW (a TOCTOU window remains between the check and the open),
    // but it's the best we can do without Win32 CreateFileW with
    // FILE_FLAG_OPEN_REPARSE_POINT.
    #[cfg(windows)]
    {
        match fs::symlink_metadata(log_path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "updater log is a symlink; refusing to open",
                ));
            },
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}, // File doesn't exist or is not a symlink — proceed
        }
    }
    let mut f = opts.open(log_path)?;

    // `OpenOptions::mode()` only applies on file *creation*. Force-set the
    // policy mode on the open fd so a pre-existing updater log converges
    // between policies on reopen (e.g. 0640 from a prior hardened run heals
    // to 0644 under the default), matching the other three log openers.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if restrict_log { 0o640 } else { 0o644 };
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }

    writeln!(
        f,
        "[{ts}] {cmd} {target} exit={code}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        ts = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ"),
        target = target_path.display(),
        code = output.status.code().map_or_else(|| "signal".to_string(), |c| c.to_string()),
        stdout = String::from_utf8_lossy(&output.stdout),
        stderr = String::from_utf8_lossy(&output.stderr),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_bucket_name_formats_correctly() {
        assert_eq!(s3_bucket_name("us-east-1"), "aws-codedeploy-us-east-1");
        assert_eq!(s3_bucket_name("eu-west-1"), "aws-codedeploy-eu-west-1");
        assert_eq!(s3_bucket_name("ap-southeast-2"), "aws-codedeploy-ap-southeast-2");
    }

    #[test]
    fn debug_impl_works() {
        let cmd = UpdateAgentCommand::new("us-east-1".into(), None);
        let debug = format!("{cmd:?}");
        assert!(debug.contains("UpdateAgentCommand"));
        assert!(debug.contains("us-east-1"));
    }

    #[test]
    fn execute_without_s3_client_errors() {
        let cmd = UpdateAgentCommand::new("us-east-1".into(), None);
        let err = cmd.execute().unwrap_err();
        assert!(err.to_string().contains("S3 client not configured"));
    }

    #[test]
    fn execute_script_missing_file_errors() {
        let err = execute_script(Path::new("/nonexistent/install"), "us-east-1").unwrap_err();
        assert!(err.to_string().contains("Install script not found"));
    }

    fn fake_output(code: i32, stdout: &str, stderr: &str) -> Output {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        Output {
            // Unix: waitpid(2) encodes the exit code in bits 15:8 of the raw status.
            #[cfg(unix)]
            status: std::process::ExitStatus::from_raw(code << 8),
            #[cfg(windows)]
            status: std::process::ExitStatus::from_raw(code as u32),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn interpret_status_ok_on_success() {
        let out = fake_output(0, "all good", "");
        assert!(interpret_status(&out).is_ok());
    }

    #[test]
    fn interpret_status_errors_on_failure_with_exit_code_and_stderr() {
        let out = fake_output(7, "", "curl or wget is required but neither was found");
        let err = interpret_status(&out).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exit 7"), "expected exit code in message, got: {msg}");
        assert!(
            msg.contains("curl or wget is required"),
            "expected stderr in message, got: {msg}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_script_runs_shell_and_returns_output() {
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("install");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&script)
            .unwrap();
        writeln!(f, "#!/bin/sh\necho \"hello from $1\"\nexit 0").unwrap();
        drop(f);

        let output = execute_script(&script, "us-west-2").unwrap();
        assert!(output.status.success());
        // The script echoes its first arg, which must be the package type.
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello from auto");
    }

    #[cfg(unix)]
    #[test]
    fn execute_script_exports_region_and_passes_auto_arg() {
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("install");
        let marker = tmp.path().join("marker");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&script)
            .unwrap();
        // Record the package-type arg and AWS_REGION the agent passed in.
        writeln!(f, "#!/bin/sh\nprintf '%s %s' \"$1\" \"$AWS_REGION\" > \"{}\"", marker.display())
            .unwrap();
        drop(f);

        let output = execute_script(&script, "eu-central-1").unwrap();
        assert!(output.status.success());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "auto eu-central-1");
    }

    #[cfg(unix)]
    #[test]
    fn strip_dynamic_loader_vars_removes_loader_env_from_child() {
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("install");
        let marker = tmp.path().join("marker");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&script)
            .unwrap();
        // Report the loader vars the script sees; all must be empty.
        writeln!(
            f,
            "#!/bin/sh\nprintf '[%s][%s][%s]' \
             \"$LD_PRELOAD\" \"$LD_LIBRARY_PATH\" \"$LD_AUDIT\" > \"{}\"",
            marker.display()
        )
        .unwrap();
        drop(f);

        // Seed the loader vars directly on the Command (simulating inheritance
        // from the agent env) without mutating the test process's own env —
        // the crate forbids unsafe, so std::env::set_var is unavailable.
        let mut cmd = Command::new(SHELL);
        cmd.arg(&script)
            .env("LD_PRELOAD", "/tmp/evil.so")
            .env("LD_LIBRARY_PATH", "/tmp/evil")
            .env("LD_AUDIT", "/tmp/audit.so");

        strip_dynamic_loader_vars(&mut cmd);

        let output = cmd.output().unwrap();
        assert!(output.status.success());
        assert_eq!(std::fs::read_to_string(&marker).unwrap(), "[][][]");
    }

    #[cfg(unix)]
    #[test]
    fn execute_script_propagates_nonzero_exit() {
        use std::os::unix::fs::OpenOptionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let script = tmp.path().join("install");
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&script)
            .unwrap();
        writeln!(f, "#!/bin/sh\necho boom >&2\nexit 3").unwrap();
        drop(f);

        let output = execute_script(&script, "us-east-1").unwrap();
        assert_eq!(output.status.code(), Some(3));
        let err = interpret_status(&output).unwrap_err();
        assert!(err.to_string().contains("exit 3"));
    }

    #[test]
    fn updater_log_creates_missing_parent_dir_and_file() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("nested/dir/updater.log");
        let out = fake_output(0, "ok", "");

        append_updater_log(&log, "/bin/sh", Path::new("/tmp/install"), &out, false).unwrap();

        let contents = std::fs::read_to_string(&log).unwrap();
        assert!(contents.contains("/bin/sh /tmp/install exit=0"));
        assert!(contents.contains("--- stdout ---\nok"));
    }

    #[cfg(unix)]
    #[test]
    fn updater_log_mode_converges_between_policies_on_reopen() {
        // `OpenOptions::mode()` only applies on creation; the explicit
        // set_permissions after open must heal a pre-existing file to the
        // current policy in both directions.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("updater.log");
        let out = fake_output(0, "ok", "");

        append_updater_log(&log, "/bin/sh", Path::new("/a"), &out, true).unwrap();
        let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "hardened updater log must be 0640, got {mode:#o}");

        append_updater_log(&log, "/bin/sh", Path::new("/b"), &out, false).unwrap();
        let mode = std::fs::metadata(&log).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "expected 0640 -> 0644 heal on default reopen, got {mode:#o}");
    }

    #[test]
    fn updater_log_appends_multiple_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("updater.log");
        let out1 = fake_output(0, "first", "");
        let out2 = fake_output(1, "", "boom");

        append_updater_log(&log, "/bin/sh", Path::new("/a"), &out1, false).unwrap();
        append_updater_log(&log, "/bin/sh", Path::new("/b"), &out2, false).unwrap();

        let contents = std::fs::read_to_string(&log).unwrap();
        assert!(contents.contains("first"));
        assert!(contents.contains("boom"));
        assert!(contents.contains("exit=0"));
        assert!(contents.contains("exit=1"));
    }

    #[cfg(unix)]
    #[test]
    fn updater_log_refuses_to_follow_symlink() {
        use std::os::unix::fs::symlink;
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("victim");
        std::fs::write(&target, "original").unwrap();
        let link = tmp.path().join("updater.log");
        symlink(&target, &link).unwrap();

        let out = fake_output(0, "x", "");
        let err = append_updater_log(&link, "/bin/sh", Path::new("/a"), &out, false).unwrap_err();
        // O_NOFOLLOW returns ELOOP when the final component is a symlink.
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "original");
        assert!(
            matches!(err.raw_os_error(), Some(libc_err) if libc_err == nix::libc::ELOOP),
            "expected ELOOP, got {err:?}"
        );
    }
}
