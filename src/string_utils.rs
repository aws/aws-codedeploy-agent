//! @risk none
//!
//! String utility functions matching the Ruby `CodeDeploy` agent.
//!
//! Ruby reference: `lib/instance_agent/string_utils.rb` (lines 1-16, branch 1.9)

/// Converts a `PascalCase` or `camelCase` string to `snake_case`.
///
/// Replicates the Ruby `StringUtils.underscore` behavior:
/// 1. `gsub(/([A-Z0-9]+)([A-Z][a-z])/, '\1_\2')` — split runs of caps before a cap+lower
/// 2. `scan(/[a-z0-9]+|\d+|[A-Z0-9]+[a-z]*/)` — extract tokens
/// 3. `join('_').downcase` — join and lowercase
#[must_use]
pub fn underscore(s: &str) -> String {
    // Step 1: insert '_' between a run of uppercase/digits and an uppercase+lowercase pair.
    // e.g. "HTMLParser" → "HTML_Parser", "ABCDef" → "ABC_Def"
    let expanded = gsub_caps(s);

    // Step 2: scan for tokens matching [a-z0-9]+ | \d+ | [A-Z0-9]+[a-z]*
    let tokens = scan_tokens(&expanded);

    // Step 3: join with '_' and downcase
    tokens.join("_").to_lowercase()
}

/// Returns `true` if the string is in `PascalCase` format.
///
/// Matches Ruby: `!!(string =~ /^([A-Z][a-z0-9]+)+/)`
/// Each word must start with an uppercase letter followed by one or more lowercase/digit chars.
/// Uses prefix matching (not full-string), matching Ruby's `=~` behavior.
#[must_use]
pub fn is_pascal_case(s: &str) -> bool {
    let bytes = s.as_bytes();
    let len = bytes.len();
    if len == 0 {
        return false;
    }

    let mut i = 0;
    let mut words = 0;

    while i < len {
        if !bytes[i].is_ascii_uppercase() {
            break;
        }
        i += 1;

        // Need at least one lowercase/digit after the uppercase
        let start = i;
        while i < len && (bytes[i].is_ascii_lowercase() || bytes[i].is_ascii_digit()) {
            i += 1;
        }
        if i == start {
            break;
        }
        words += 1;
    }

    words > 0
}

/// Step 1: replicate `gsub(/([A-Z0-9]+)([A-Z][a-z])/, '\1_\2')`
///
/// Inserts '_' when a run of uppercase/digit chars is followed by an uppercase+lowercase pair.
fn gsub_caps(s: &str) -> String {
    let bytes = s.as_bytes();
    let len = bytes.len();
    // Room for a few inserted underscores at cap boundaries
    let mut out = String::with_capacity(len + 4);

    let mut i = 0;
    while i < len {
        let b = bytes[i];
        // Check if we need to insert an underscore before this character.
        // Condition: current char is uppercase, next char is lowercase,
        // and previous char is uppercase or digit (i.e., we're at the boundary).
        if i > 0
            && b.is_ascii_uppercase()
            && i + 1 < len
            && bytes[i + 1].is_ascii_lowercase()
            && (bytes[i - 1].is_ascii_uppercase() || bytes[i - 1].is_ascii_digit())
        {
            out.push('_');
        }
        out.push(b as char);
        i += 1;
    }
    out
}

/// Step 2: replicate `scan(/[a-z0-9]+|\d+|[A-Z0-9]+[a-z]*/)`
///
/// The Ruby regex has a `\d+` alternative, but it's redundant — digits are already
/// matched by `[a-z0-9]+` which Ruby tries first. We implement two branches instead of three.
fn scan_tokens(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut tokens = Vec::new();
    let mut i = 0;

    while i < len {
        let b = bytes[i];

        if b.is_ascii_lowercase() || b.is_ascii_digit() {
            // Match [a-z0-9]+
            let start = i;
            while i < len && (bytes[i].is_ascii_lowercase() || bytes[i].is_ascii_digit()) {
                i += 1;
            }
            tokens.push(&s[start..i]);
        } else if b.is_ascii_uppercase() {
            // Match [A-Z0-9]+[a-z]*
            let start = i;
            while i < len && (bytes[i].is_ascii_uppercase() || bytes[i].is_ascii_digit()) {
                i += 1;
            }
            while i < len && bytes[i].is_ascii_lowercase() {
                i += 1;
            }
            tokens.push(&s[start..i]);
        } else {
            // Skip non-matching characters (underscores, hyphens, etc.)
            i += 1;
        }
    }

    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- underscore tests ---

    #[test]
    fn underscore_converts_download_bundle() {
        assert_eq!(underscore("DownloadBundle"), "download_bundle");
    }

    #[test]
    fn underscore_converts_install() {
        assert_eq!(underscore("Install"), "install");
    }

    #[test]
    fn underscore_converts_after_allow_traffic() {
        assert_eq!(underscore("AfterAllowTraffic"), "after_allow_traffic");
    }

    #[test]
    fn underscore_converts_after_allow_test_traffic() {
        assert_eq!(underscore("AfterAllowTestTraffic"), "after_allow_test_traffic");
    }

    #[test]
    fn underscore_converts_camel_case() {
        assert_eq!(underscore("downloadBundle"), "download_bundle");
    }

    #[test]
    fn underscore_preserves_snake_case() {
        assert_eq!(underscore("download_bundle"), "download_bundle");
    }

    #[test]
    fn underscore_splits_consecutive_caps() {
        assert_eq!(underscore("HTMLParser"), "html_parser");
    }

    #[test]
    fn underscore_converts_before_block_traffic() {
        assert_eq!(underscore("BeforeBlockTraffic"), "before_block_traffic");
    }

    #[test]
    fn underscore_handles_all_lifecycle_events() {
        assert_eq!(underscore("BeforeBlockTraffic"), "before_block_traffic");
        assert_eq!(underscore("AfterBlockTraffic"), "after_block_traffic");
        assert_eq!(underscore("ApplicationStop"), "application_stop");
        assert_eq!(underscore("BeforeInstall"), "before_install");
        assert_eq!(underscore("AfterInstall"), "after_install");
        assert_eq!(underscore("ApplicationStart"), "application_start");
        assert_eq!(underscore("BeforeAllowTraffic"), "before_allow_traffic");
        assert_eq!(underscore("AfterAllowTraffic"), "after_allow_traffic");
        assert_eq!(underscore("ValidateService"), "validate_service");
    }

    #[test]
    fn underscore_handles_empty_string() {
        assert_eq!(underscore(""), "");
    }

    #[test]
    fn underscore_handles_single_word() {
        assert_eq!(underscore("Word"), "word");
    }

    // --- is_pascal_case tests ---

    #[test]
    fn pascal_case_accepts_download_bundle() {
        assert!(is_pascal_case("DownloadBundle"));
    }

    #[test]
    fn pascal_case_accepts_install() {
        assert!(is_pascal_case("Install"));
    }

    #[test]
    fn pascal_case_rejects_all_caps() {
        assert!(!is_pascal_case("DOWNLOADBUNDLE"));
    }

    #[test]
    fn pascal_case_rejects_camel_case() {
        assert!(!is_pascal_case("downloadBundle"));
    }

    #[test]
    fn pascal_case_rejects_all_lowercase() {
        assert!(!is_pascal_case("downloadbundle"));
    }

    #[test]
    fn pascal_case_rejects_empty_string() {
        assert!(!is_pascal_case(""));
    }

    #[test]
    fn pascal_case_rejects_single_uppercase() {
        assert!(!is_pascal_case("A"));
    }

    #[test]
    fn pascal_case_accepts_minimal() {
        assert!(is_pascal_case("Ab"));
    }

    // Ruby test: test_is_camel_case_second_uppercase ("downloadbUndle")
    #[test]
    fn pascal_case_rejects_mid_word_uppercase() {
        assert!(!is_pascal_case("downloadbUndle"));
    }

    // --- gsub_caps internal tests ---

    #[test]
    fn gsub_caps_inserts_underscore_before_cap_lower_boundary() {
        assert_eq!(gsub_caps("HTMLParser"), "HTML_Parser");
    }

    #[test]
    fn gsub_caps_no_change_for_simple_pascal() {
        assert_eq!(gsub_caps("DownloadBundle"), "DownloadBundle");
    }

    // --- scan_tokens internal tests ---

    #[test]
    fn scan_tokens_splits_pascal_case() {
        assert_eq!(scan_tokens("DownloadBundle"), vec!["Download", "Bundle"]);
    }

    #[test]
    fn scan_tokens_handles_underscore_separator() {
        assert_eq!(scan_tokens("download_bundle"), vec!["download", "bundle"]);
    }
}
