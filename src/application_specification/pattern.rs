//! Glob pattern matching for file mappings.
//!
//! Backed by [`globset`] (DFA-backed, linear-time matching). Replaces the
//! previous hand-rolled `simple_glob_match` which had exponential worst-case
//! behavior on patterns with multiple `*`s (CWE-1333).

use globset::{Glob, GlobMatcher};

#[derive(Debug, Clone)]
pub(crate) enum GlobPattern {
    MatchAll,
    Exact(String),
    Wildcard(GlobMatcher),
}

impl PartialEq for GlobPattern {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (GlobPattern::MatchAll, GlobPattern::MatchAll) => true,
            (GlobPattern::Exact(a), GlobPattern::Exact(b)) => a == b,
            (GlobPattern::Wildcard(a), GlobPattern::Wildcard(b)) => a.glob() == b.glob(),
            _ => false,
        }
    }
}

impl GlobPattern {
    pub(crate) fn compile(pattern: &str) -> Self {
        if pattern == "**" {
            return GlobPattern::MatchAll;
        }
        if !pattern.contains('*') {
            return GlobPattern::Exact(pattern.to_string());
        }
        match Glob::new(pattern) {
            Ok(glob) => GlobPattern::Wildcard(glob.compile_matcher()),
            Err(_) => GlobPattern::Exact(pattern.to_string()),
        }
    }

    /// Returns the original source string of this pattern.
    ///
    /// Used to surface the user's own glob in error messages instead of the
    /// compiled matcher's `Debug` output, which leaks ~4KB of regex-automata
    /// internals (NFA/DFA/PikeVM state tables) into user-visible errors.
    pub(crate) fn as_str(&self) -> &str {
        match self {
            GlobPattern::MatchAll => "**",
            GlobPattern::Exact(s) => s,
            GlobPattern::Wildcard(matcher) => matcher.glob().glob(),
        }
    }

    /// Checks if a filename matches this glob pattern.
    ///
    /// `Wildcard` patterns use a pre-compiled [`GlobMatcher`]
    /// (linear-time regardless of `*` count).
    ///
    /// Note: Currently unused in production; the installer only validates
    /// that permissions use `MatchAll` (**). Tested for future use.
    #[allow(dead_code)]
    pub(crate) fn matches(&self, name: &str) -> bool {
        match self {
            GlobPattern::MatchAll => true,
            GlobPattern::Exact(s) => name == s,
            GlobPattern::Wildcard(matcher) => {
                if name.contains('/') || name.contains('\\') {
                    return false;
                }
                matcher.is_match(name)
            },
        }
    }
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
        assert!(matches!(pattern, GlobPattern::Wildcard(_)));
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
        let pattern = GlobPattern::compile("*.txt");
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches("test.txt"));
        assert!(!pattern.matches("file.log"));
    }

    #[test]
    fn glob_matches_wildcard_end() {
        let pattern = GlobPattern::compile("file*");
        assert!(pattern.matches("file"));
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches("filename"));
        assert!(!pattern.matches("other"));
    }

    #[test]
    fn glob_matches_wildcard_middle() {
        let pattern = GlobPattern::compile("file*.txt");
        assert!(pattern.matches("file.txt"));
        assert!(pattern.matches("filename.txt"));
        assert!(!pattern.matches("file.log"));
    }

    #[test]
    fn glob_matches_multiple_wildcards() {
        let pattern = GlobPattern::compile("*file*txt*");
        assert!(pattern.matches("myfiletxt"));
        assert!(pattern.matches("file.txt.bak"));
        assert!(!pattern.matches("other"));
    }

    #[test]
    fn glob_matches_rejects_path_separators() {
        let pattern = GlobPattern::compile("*.txt");
        assert!(!pattern.matches("dir/file.txt"));
        assert!(!pattern.matches("dir\\file.txt"));
    }

    #[test]
    fn glob_matches_empty_pattern() {
        let pattern = GlobPattern::compile("*");
        assert!(pattern.matches("file"));
        assert!(pattern.matches(""));
    }

    #[test]
    fn glob_matches_complex_pattern() {
        let pattern = GlobPattern::compile("a*b*c");
        assert!(pattern.matches("abc"));
        assert!(pattern.matches("aXbYc"));
        assert!(!pattern.matches("abcd"));
    }

    #[test]
    fn glob_match_completes_quickly_on_redos_shaped_pattern() {
        // Regression test: 10K-char pattern previously took ~40s in the
        // exponential matcher. globset is linear-time.
        let pattern_str = format!("{}!", "a".repeat(10_000));
        let pattern = GlobPattern::compile(&pattern_str);
        let input = "a".repeat(10_000);

        let start = std::time::Instant::now();
        let result = pattern.matches(&input);
        let elapsed = start.elapsed();

        assert!(!result, "trailing '!' must not match input without '!'");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "match should be linear-time; took {elapsed:?}"
        );
    }

    #[test]
    fn glob_match_completes_quickly_on_redos_shaped_star_pattern() {
        // Adversarial shape: many `*a` repeats with a non-matching trailing
        // literal. Old matcher branched exponentially on every `*`.
        let pattern_str = format!("{}!", "*a".repeat(50));
        let pattern = GlobPattern::compile(&pattern_str);
        let input = "a".repeat(100);

        let start = std::time::Instant::now();
        let result = pattern.matches(&input);
        let elapsed = start.elapsed();

        assert!(!result, "trailing '!' never matches");
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "exponential-shape pattern should still match in linear time; took {elapsed:?}"
        );
    }

    #[test]
    fn wildcard_partial_eq_same_glob() {
        let a = GlobPattern::compile("*.txt");
        let b = GlobPattern::compile("*.txt");
        assert_eq!(a, b);
    }

    #[test]
    fn wildcard_partial_eq_different_glob() {
        let a = GlobPattern::compile("*.txt");
        let b = GlobPattern::compile("*.log");
        assert_ne!(a, b);
    }

    #[test]
    fn partial_eq_wildcard_vs_exact_is_false() {
        let wildcard = GlobPattern::compile("*.txt");
        let exact = GlobPattern::Exact("*.txt".to_string());
        assert_ne!(wildcard, exact);
    }

    #[test]
    fn partial_eq_exact_vs_match_all_is_false() {
        let exact = GlobPattern::Exact("**".to_string());
        let match_all = GlobPattern::MatchAll;
        assert_ne!(exact, match_all);
    }

    #[test]
    fn compile_invalid_glob_falls_back_to_exact() {
        let pattern = GlobPattern::compile("[unclosed");
        assert_eq!(pattern, GlobPattern::Exact("[unclosed".to_string()));
    }
}
