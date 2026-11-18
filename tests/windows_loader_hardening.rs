//! The agent binary must resolve its DLLs from `System32` only, whatever sits
//! in its own folder. See `src/loader_hardening.rs`.
#![cfg(windows)]

use std::path::Path;
use std::process::Command;

const AGENT: &str = env!("CARGO_BIN_EXE_codedeploy-agent");
/// `LOAD_LIBRARY_SEARCH_SYSTEM32`.
const SYSTEM32_ONLY: u16 = 0x800;
/// A static import of the agent that is not one of the `KnownDLLs`.
const IMPORTED_DLL: &str = "userenv.dll";

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// File offset of `DependentLoadFlags` in the PE32+ load configuration, or
/// `None` if the image has no load configuration covering it.
fn dependent_load_flags_offset(image: &[u8]) -> Option<usize> {
    let pe = u32_at(image, 0x3C) as usize;
    assert_eq!(&image[pe..pe + 4], b"PE\0\0", "not a PE image");
    let opt = pe + 24;
    assert_eq!(u16_at(image, opt), 0x20B, "not PE32+");
    // DataDirectory[10] is the load configuration.
    let (rva, size) = (u32_at(image, opt + 112 + 80), u32_at(image, opt + 112 + 84));
    if rva == 0 || size == 0 {
        return None;
    }
    let table = opt + usize::from(u16_at(image, pe + 20));
    let offset = (0..usize::from(u16_at(image, pe + 6))).map(|i| table + i * 40).find_map(|s| {
        let (va, raw_size, raw_ptr) =
            (u32_at(image, s + 12), u32_at(image, s + 16), u32_at(image, s + 20));
        (rva >= va && rva < va + raw_size).then(|| (rva - va + raw_ptr) as usize)
    })?;
    // DependentLoadFlags sits at offset 78 of IMAGE_LOAD_CONFIG_DIRECTORY64.
    (u32_at(image, offset) >= 80).then_some(offset + 78)
}

fn copy_agent_to(dir: &Path) -> std::path::PathBuf {
    let exe = dir.join("codedeploy-agent.exe");
    std::fs::copy(AGENT, &exe).unwrap();
    exe
}

fn runs(exe: &Path) -> bool {
    Command::new(exe).arg("--version").output().is_ok_and(|o| o.status.success())
}

#[test]
fn image_restricts_static_imports_to_system32() {
    let image = std::fs::read(AGENT).unwrap();
    let field = dependent_load_flags_offset(&image).expect("image has no load configuration");
    assert_eq!(u16_at(&image, field), SYSTEM32_ONLY);
}

#[test]
fn agent_ignores_a_dll_in_its_own_folder() {
    let dir = tempfile::tempdir().unwrap();
    let exe = copy_agent_to(dir.path());
    // Not a valid image: loading it would fail the process start.
    std::fs::write(dir.path().join(IMPORTED_DLL), b"MZ").unwrap();
    assert!(runs(&exe), "agent failed to start with {IMPORTED_DLL} beside it");
}

#[test]
fn agent_ignores_a_dot_local_folder() {
    let dir = tempfile::tempdir().unwrap();
    let exe = copy_agent_to(dir.path());
    let redirect = dir.path().join("codedeploy-agent.exe.local");
    std::fs::create_dir(&redirect).unwrap();
    std::fs::write(redirect.join(IMPORTED_DLL), b"MZ").unwrap();
    assert!(runs(&exe), "agent failed to start with {IMPORTED_DLL} in a .local folder");
}

#[test]
fn control_an_unhardened_copy_loads_the_dll_beside_it() {
    // With DependentLoadFlags cleared in a copy, the planted DLL must break the
    // start; otherwise the tests above prove nothing on this host.
    let dir = tempfile::tempdir().unwrap();
    let mut image = std::fs::read(AGENT).unwrap();
    let field = dependent_load_flags_offset(&image).expect("image has no load configuration");
    image[field..field + 2].copy_from_slice(&0u16.to_le_bytes());
    let exe = dir.path().join("codedeploy-agent.exe");
    std::fs::write(&exe, &image).unwrap();
    std::fs::write(dir.path().join(IMPORTED_DLL), b"MZ").unwrap();
    assert!(!runs(&exe), "unhardened copy started despite a bad {IMPORTED_DLL} beside it");
}
