//! `AppSpec` file mapping types.
use crate::application_specification::ParseError;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Files(Vec<FileMapping>);

impl Files {
    pub(crate) fn new(vec: Vec<FileMapping>) -> Self {
        Files(vec)
    }

    pub fn iter(&self) -> impl Iterator<Item = &FileMapping> {
        self.0.iter()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileMapping {
    source: String,
    destination: String,
}

impl FileMapping {
    pub(crate) fn new(source: String, destination: String) -> Result<Self, ParseError> {
        if source.is_empty() {
            return Err(ParseError::MissingSource);
        }
        if destination.is_empty() {
            return Err(ParseError::MissingDestination(source));
        }
        Ok(FileMapping { source, destination })
    }

    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    #[must_use]
    pub fn destination(&self) -> &str {
        &self.destination
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::{AppSpec, ParseError};

    #[test]
    fn files_via_appspec() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: /dest\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.files().iter().count(), 1);
    }

    #[test]
    fn file_mapping_source() {
        let yaml =
            "version: 0.0\nos: linux\nfiles:\n  - source: /source/path\n    destination: /dest\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let file = spec.files().iter().next().unwrap();
        assert_eq!(file.source(), "/source/path");
    }

    #[test]
    fn file_mapping_destination() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: /destination/path\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let file = spec.files().iter().next().unwrap();
        assert_eq!(file.destination(), "/destination/path");
    }

    #[test]
    fn file_mapping_missing_source() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: \"\"\n    destination: /dest\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::MissingSource)));
    }

    #[test]
    fn file_mapping_missing_destination() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: \"\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::MissingDestination(_))));
    }

    #[test]
    fn file_mapping_new_empty_source() {
        let result = FileMapping::new(String::new(), "/dest".to_string());
        assert!(matches!(result, Err(ParseError::MissingSource)));
    }

    #[test]
    fn file_mapping_new_empty_destination() {
        let result = FileMapping::new("/src".to_string(), String::new());
        assert!(matches!(result, Err(ParseError::MissingDestination(_))));
    }
}
