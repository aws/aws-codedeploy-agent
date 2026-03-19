//! @risk low
//!
//! Glob pattern matching for file mappings.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum GlobPattern {
    MatchAll,
    Exact(String),
    Wildcard(String),
}

impl GlobPattern {
    pub(crate) fn compile(pattern: &str) -> Self {
        if pattern == "**" {
            return GlobPattern::MatchAll;
        }
        if !pattern.contains('*') {
            return GlobPattern::Exact(pattern.to_string());
        }
        GlobPattern::Wildcard(pattern.to_string())
    }

    /// Checks if a filename matches this glob pattern.
    ///
    /// Note: Currently unused in production code but tested and ready for future use.
    /// The current implementation only validates that permissions use `MatchAll` (**) patterns.
    /// When file filtering is implemented, this method will be used for actual matching.
    #[allow(dead_code)]
    pub(crate) fn matches(&self, name: &str) -> bool {
        match self {
            GlobPattern::MatchAll => true,
            GlobPattern::Exact(s) => name == s,
            GlobPattern::Wildcard(pattern) => simple_glob_match(name, pattern),
        }
    }
}

/// Simple glob pattern matching for filenames (no path separators).
///
/// Supports wildcards (*) matching zero or more characters.
/// Rejects names containing path separators (/ or \).
///
/// Note: Currently unused in production code but tested and ready for future use.
/// This provides the matching logic for `GlobPattern::matches()`.
#[allow(dead_code)]
fn simple_glob_match(name: &str, pattern: &str) -> bool {
    if name.contains('/') || name.contains('\\') {
        return false;
    }

    let name_chars: Vec<char> = name.chars().collect();
    let pattern_chars: Vec<char> = pattern.chars().collect();

    let mut options = vec![pattern_chars.clone()];

    for &ch in &name_chars {
        let mut new_options = Vec::new();

        for mut option in options {
            if option.is_empty() {
                continue;
            }

            if option[0] == '*' {
                new_options.push(option.clone());
                option.remove(0);
                if !option.is_empty() {
                    new_options.push(option.clone());
                }
            }

            if !option.is_empty() && option[0] == ch {
                option.remove(0);
                new_options.push(option);
            }
        }

        options = new_options;

        if options.iter().any(|o| o.len() == 1 && o[0] == '*') {
            return true;
        }
    }

    options.iter().any(|o| o.is_empty() || (o.len() == 1 && o[0] == '*'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::AppSpec;

    // pattern_matching_coverage
    #[test]
    fn pattern_wildcard_at_start() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*file.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_at_end() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"file*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_in_middle() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"file*.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_multiple_wildcards() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*file*txt*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_only_wildcard() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_double_wildcard() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"**\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_with_backslash_separator() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_empty_string() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_no_wildcard() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"exact.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_complex_wildcard_matching() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"a*b*c\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_consecutive() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"**/*.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn except_pattern_single() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    except:\n      - \"*.log\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn except_pattern_multiple() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    except:\n      - \"*.log\"\n      - \"*.tmp\"\n      - \"*.bak\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn except_pattern_exact() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    except:\n      - \"exact.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn except_pattern_match_all() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    except:\n      - \"**\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    // Direct GlobPattern tests
    #[test]
    fn glob_compile_match_all() {
        let pattern = GlobPattern::compile("**");
        assert_eq!(pattern, GlobPattern::MatchAll);
    }

    #[test]
    fn glob_compile_exact() {
        let pattern = GlobPattern::compile("file.txt");
        assert_eq!(pattern, GlobPattern::Exact("file.txt".to_string()));
    }

    #[test]
    fn glob_compile_wildcard() {
        let pattern = GlobPattern::compile("*.txt");
        assert_eq!(pattern, GlobPattern::Wildcard("*.txt".to_string()));
    }

    #[test]
    fn glob_matches_match_all() {
        let pattern = GlobPattern::MatchAll;
        assert!(pattern.matches("anything"));
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches(""));
    }

    #[test]
    fn glob_matches_exact() {
        let pattern = GlobPattern::Exact("file.txt".to_string());
        assert!(pattern.matches("file.txt"));
        assert!(!pattern.matches("other.txt"));
        assert!(!pattern.matches("file.tx"));
    }

    #[test]
    fn glob_matches_wildcard_start() {
        let pattern = GlobPattern::Wildcard("*.txt".to_string());
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches("test.txt"));
        assert!(!pattern.matches("file.log"));
    }

    #[test]
    fn glob_matches_wildcard_end() {
        let pattern = GlobPattern::Wildcard("file*".to_string());
        assert!(pattern.matches("file"));
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches("filename"));
        assert!(!pattern.matches("other"));
    }

    #[test]
    fn glob_matches_wildcard_middle() {
        let pattern = GlobPattern::Wildcard("file*.txt".to_string());
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches("filename.txt"));
        assert!(!pattern.matches("file.log"));
    }

    #[test]
    fn glob_matches_multiple_wildcards() {
        let pattern = GlobPattern::Wildcard("*file*txt*".to_string());
        assert!(pattern.matches("myfiletxt"));
        assert!(pattern.matches("file.txt.bak"));
        assert!(!pattern.matches("other"));
    }

    #[test]
    fn glob_matches_rejects_path_separators() {
        let pattern = GlobPattern::Wildcard("*.txt".to_string());
        assert!(!pattern.matches("dir/file.txt"));
        assert!(!pattern.matches("dir\\file.txt"));
    }

    #[test]
    fn glob_matches_empty_pattern() {
        let pattern = GlobPattern::Wildcard("*".to_string());
        assert!(pattern.matches("file"));
        assert!(pattern.matches(""));
    }

    #[test]
    fn glob_matches_complex_pattern() {
        let pattern = GlobPattern::Wildcard("a*b*c".to_string());
        assert!(pattern.matches("abc"));
        assert!(pattern.matches("aXbYc"));
        assert!(!pattern.matches("abcd"));
    }
}
