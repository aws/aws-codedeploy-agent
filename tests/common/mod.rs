use std::path::{Path, PathBuf};

#[allow(dead_code)]
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

#[allow(dead_code)]
pub fn appspec_fixture(name: &str) -> PathBuf {
    fixtures_dir().join("appspec").join(name)
}

#[allow(dead_code)]
pub fn deployment_spec_fixture(name: &str) -> PathBuf {
    fixtures_dir().join("deployment_spec").join(name)
}

#[allow(dead_code)]
pub fn bundle_fixture(name: &str) -> PathBuf {
    fixtures_dir().join("bundles").join(name)
}

#[allow(dead_code)]
pub fn read_fixture(path: &Path) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {}: {e}", path.display()))
}
