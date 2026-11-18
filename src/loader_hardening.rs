//! Windows DLL search hardening for the agent binary.
//!
//! Restricts where Windows resolves the agent's DLLs to `System32`, so the
//! agent never loads a DLL from its own install folder:
//!
//! - Static imports are resolved before `main` runs. Their search path is set
//!   by `DependentLoadFlags` in the PE load configuration, which the MSVC
//!   linker writes for `/DEPENDENTLOADFLAG`. The release build links with GNU
//!   ld, which has no such option but emits the load configuration directory
//!   from a `_load_config_used` symbol, so the binary defines one.
//! - DLLs loaded at runtime follow the process default set by
//!   [`restrict_dll_search`], called first thing in `main`.

use windows_sys::Win32::System::Diagnostics::Debug::IMAGE_LOAD_CONFIG_DIRECTORY64;
use windows_sys::Win32::System::LibraryLoader::{
    LOAD_LIBRARY_SEARCH_SYSTEM32, SetDefaultDllDirectories,
};

/// `windows-sys` declares the directory `packed(4)`; GNU ld requires the
/// symbol to be 8-byte aligned on x64.
#[repr(C, align(8))]
pub struct LoadConfig(IMAGE_LOAD_CONFIG_DIRECTORY64);

/// PE load configuration for the agent binary. Every field but `Size` and
/// `DependentLoadFlags` is zero, which leaves the corresponding feature off.
#[allow(unsafe_code, non_upper_case_globals, clippy::cast_possible_truncation)]
#[used]
#[unsafe(no_mangle)]
pub static _load_config_used: LoadConfig = LoadConfig(IMAGE_LOAD_CONFIG_DIRECTORY64 {
    Size: size_of::<IMAGE_LOAD_CONFIG_DIRECTORY64>() as u32,
    DependentLoadFlags: LOAD_LIBRARY_SEARCH_SYSTEM32 as u16,
    // SAFETY: the struct is plain integers; all-zero is a valid value.
    ..unsafe { core::mem::zeroed() }
});

/// Make `System32` the only default search location for DLLs this process
/// loads at runtime.
#[allow(unsafe_code)]
pub fn restrict_dll_search() {
    // SAFETY: takes a flags value and has no other preconditions.
    // A failure leaves the default search order; nothing else depends on it.
    let _ = unsafe { SetDefaultDllDirectories(LOAD_LIBRARY_SEARCH_SYSTEM32) };
}
