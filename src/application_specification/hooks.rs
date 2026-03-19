//! @risk low
//!
//! `AppSpec` lifecycle hook types.
use crate::application_specification::ParseError;
use indexmap::IndexMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hooks(IndexMap<String, Vec<ScriptInfo>>);

impl Hooks {
    pub(crate) fn new(map: IndexMap<String, Vec<ScriptInfo>>) -> Self {
        Hooks(map)
    }

    #[must_use]
    pub fn get(&self, event: &str) -> &[ScriptInfo] {
        // Look up event in IndexMap -> Option<&Vec<ScriptInfo>>
        // Convert Vec to slice -> Option<&[ScriptInfo]>
        // If None, return empty slice
        self.0.get(event).map_or(&[], std::vec::Vec::as_slice)
    }

    pub fn events(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(std::string::String::as_str)
    }

    #[must_use]
    pub fn has_event(&self, event: &str) -> bool {
        self.0.contains_key(event)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptInfo {
    location: ScriptLocation,
    runas: Option<Username>,
    sudo: Option<bool>,
    timeout: Timeout,
}

impl ScriptInfo {
    pub(crate) fn new(
        location: ScriptLocation,
        runas: Option<Username>,
        sudo: Option<bool>,
        timeout: Timeout,
    ) -> Self {
        ScriptInfo { location, runas, sudo, timeout }
    }

    #[must_use]
    pub fn location(&self) -> &str {
        self.location.as_str()
    }

    #[must_use]
    pub fn runas(&self) -> Option<&str> {
        self.runas.as_ref().map(|u| u.0.as_str())
    }

    #[must_use]
    pub fn sudo(&self) -> Option<bool> {
        self.sudo
    }

    #[must_use]
    pub fn timeout(&self) -> u32 {
        self.timeout.seconds()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScriptLocation(String);

impl ScriptLocation {
    pub(crate) fn new(s: &str) -> Result<Self, ParseError> {
        let trimmed = s.trim().to_string();
        if trimmed.is_empty() {
            Err(ParseError::EmptyScriptLocation)
        } else {
            Ok(ScriptLocation(trimmed))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Username(pub(crate) String);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timeout(u32);

impl Timeout {
    pub const DEFAULT_SECONDS: u32 = 3600;

    pub(crate) fn new(seconds: u32) -> Result<Self, ParseError> {
        if seconds == 0 {
            Err(ParseError::InvalidTimeout)
        } else {
            Ok(Timeout(seconds))
        }
    }

    #[must_use]
    pub fn seconds(&self) -> u32 {
        self.0
    }
}

impl Default for Timeout {
    fn default() -> Self {
        Timeout(Self::DEFAULT_SECONDS)
    }
}

#[cfg(test)]
mod tests {
    use crate::application_specification::AppSpec;

    #[test]
    fn hooks_via_appspec() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStop:\n    - location: stop.sh\n      timeout: 300\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStop");
        assert!(!scripts.is_empty());

        // Test location() and timeout() accessors
        let script = &scripts[0];
        assert_eq!(script.location(), "stop.sh");
        assert_eq!(script.timeout(), 300);
    }

    #[test]
    fn empty_hooks_list() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStop: []\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStop");
        assert!(scripts.is_empty());
    }
}
