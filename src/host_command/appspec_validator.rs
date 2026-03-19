//! @risk medium
//!
//! Appspec hook validation.
//!
//! Validates that lifecycle events in the appspec and hook mapping are allowed
//! by the server, and that custom lifecycle events are defined in the appspec.

use crate::application_specification::AppSpec;
use std::collections::HashSet;

/// The default ordered lifecycle events.
const DEFAULT_LIFECYCLE_EVENTS: &[&str] = &[
    "BeforeBlockTraffic",
    "AfterBlockTraffic",
    "ApplicationStop",
    "DownloadBundle",
    "Install",
    "BeforeInstall",
    "AfterInstall",
    "ApplicationStart",
    "BeforeAllowTraffic",
    "AfterAllowTraffic",
    "ValidateService",
];

/// Validate that the appspec hooks are allowed by the server.
///
/// Checks:
/// 1. All hooks in appspec + `hook_mapping` are a subset of `all_possible_events`
/// 2. Custom lifecycle events (non-default) exist in the appspec
///
/// # Errors
/// Returns an error message if validation fails.
pub fn validate_hooks(
    app_spec: &AppSpec,
    app_spec_filename: &str,
    all_possible_events: Option<&[String]>,
    hook_mapping_keys: &[String],
    default_mapping_keys: &[String],
) -> Result<(), String> {
    let Some(all_possible) = all_possible_events else {
        return Ok(());
    };

    let allowed: HashSet<&str> = all_possible.iter().map(String::as_str).collect();

    let appspec_events: HashSet<&str> = app_spec.hooks().events().collect();

    // Check all hooks from appspec + hook_mapping are in allowed set
    let combined: HashSet<&str> = appspec_events
        .iter()
        .copied()
        .chain(hook_mapping_keys.iter().map(String::as_str))
        .collect();

    let unknown: Vec<&str> = combined.difference(&allowed).copied().collect();
    if !unknown.is_empty() {
        return Err(format!(
            "{app_spec_filename} file contains unknown lifecycle events: {}",
            unknown.join(", ")
        ));
    }

    // Check custom events exist in appspec or default mapping
    let defaults: HashSet<&str> = DEFAULT_LIFECYCLE_EVENTS.iter().copied().collect();

    let known_hooks: HashSet<&str> = appspec_events
        .iter()
        .copied()
        .chain(default_mapping_keys.iter().map(String::as_str))
        .collect();

    let missing: Vec<&str> = all_possible
        .iter()
        .map(String::as_str)
        .filter(|e| !defaults.contains(e) && !known_hooks.contains(e))
        .collect();

    if !missing.is_empty() {
        return Err(format!(
            "You specified a lifecycle event which is not a default one and doesn't exist in your {app_spec_filename} file: {}",
            missing.join(",")
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_appspec(yaml: &str) -> AppSpec {
        AppSpec::parse(yaml).unwrap()
    }

    const BASIC_APPSPEC: &str = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
";

    const CUSTOM_HOOK_APPSPEC: &str = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
  MyCustomHook:
    - location: scripts/custom.sh
";

    #[test]
    fn validate_no_possible_events_always_ok() {
        let spec = parse_appspec(BASIC_APPSPEC);
        let result = validate_hooks(&spec, "appspec.yml", None, &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_all_hooks_in_allowed_set() {
        let spec = parse_appspec(BASIC_APPSPEC);
        let allowed = vec!["AfterInstall".into(), "BeforeInstall".into()];
        let result = validate_hooks(&spec, "appspec.yml", Some(&allowed), &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_unknown_hook_in_appspec() {
        let spec = parse_appspec(BASIC_APPSPEC);
        let allowed = vec!["BeforeInstall".into()]; // AfterInstall not allowed
        let result = validate_hooks(&spec, "appspec.yml", Some(&allowed), &[], &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown lifecycle events"));
    }

    #[test]
    fn validate_unknown_hook_from_mapping() {
        let spec = parse_appspec(BASIC_APPSPEC);
        let allowed = vec!["AfterInstall".into()];
        let mapping = vec!["UnknownHook".into()];
        let result = validate_hooks(&spec, "appspec.yml", Some(&allowed), &mapping, &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown lifecycle events"));
    }

    #[test]
    fn validate_custom_event_in_appspec() {
        let spec = parse_appspec(CUSTOM_HOOK_APPSPEC);
        let allowed = vec!["AfterInstall".into(), "MyCustomHook".into()];
        let result = validate_hooks(&spec, "appspec.yml", Some(&allowed), &[], &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn validate_custom_event_missing_from_appspec() {
        let spec = parse_appspec(BASIC_APPSPEC);
        let allowed = vec![
            "AfterInstall".into(),
            "MyCustomHook".into(), // custom, not in appspec
        ];
        let result = validate_hooks(&spec, "appspec.yml", Some(&allowed), &[], &[]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a default one"));
    }

    #[test]
    fn validate_custom_event_in_default_mapping() {
        let spec = parse_appspec(BASIC_APPSPEC);
        let allowed = vec!["AfterInstall".into(), "MyCustomHook".into()];
        let default_mapping = vec!["MyCustomHook".into()];
        let result = validate_hooks(&spec, "appspec.yml", Some(&allowed), &[], &default_mapping);
        assert!(result.is_ok());
    }
}
