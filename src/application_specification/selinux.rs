//! `SELinux` context types for `AppSpec` permissions.
use crate::application_specification::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub struct SeLinuxContext {
    user: Option<String>,
    role: Option<String>,
    type_: String,
    range: Option<MlsRange>,
}

impl SeLinuxContext {
    /// Creates a new `SELinux` context.
    ///
    /// Note: `role` is always None because the `CodeDeploy` agent doesn't parse or use
    /// the role field from the `AppSpec` file.
    /// implementation which only handles user, type, and range.
    pub(crate) fn new(user: Option<String>, type_: String, range: Option<MlsRange>) -> Self {
        SeLinuxContext {
            user,
            role: None, // Not parsed from AppSpec
            type_,
            range,
        }
    }

    /// Creates a new `SELinux` context with role (test-only).
    ///
    /// This is only used for testing the role validation error path.
    /// In production, role is always None.
    #[cfg(test)]
    pub(crate) fn new_with_role(
        user: Option<String>,
        role: Option<String>,
        type_: String,
        range: Option<MlsRange>,
    ) -> Self {
        SeLinuxContext { user, role, type_, range }
    }

    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }
    #[must_use]
    pub fn role(&self) -> Option<&str> {
        self.role.as_deref()
    }
    #[must_use]
    pub fn type_(&self) -> &str {
        &self.type_
    }
    #[must_use]
    pub fn range(&self) -> Option<&MlsRange> {
        self.range.as_ref()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MlsRange {
    low_sensitivity: u8,
    high_sensitivity: u8,
    categories: Option<Vec<u16>>,
}

impl MlsRange {
    /// Parses an MLS (Multi-Level Security) range string.
    ///
    /// Format: `s<low>[-s<high>][:c<cat>[,c<cat>|c<low>.c<high>]*]`
    ///
    /// Examples:
    /// - `s0` - Single sensitivity level
    /// - `s0-s15` - Sensitivity range
    /// - `s0:c0` - Sensitivity with single category
    /// - `s0:c0,c5` - Multiple categories
    /// - `s0:c0.c10` - Category range (expands to c0,c1,...,c10)
    /// - `s0-s1:c0.c5,c10` - Full range with categories
    pub(crate) fn parse(s: &str) -> Result<Self, ParseError> {
        // Split on ':' to separate sensitivity from categories
        // Format: sensitivity_part[:category_part]
        let parts: Vec<&str> = s.split(':').collect();

        // Parse sensitivity levels (s0, s0-s15, etc.)
        let sensitivity_part = parts[0];
        let sens_parts: Vec<&str> = sensitivity_part.split('-').collect();

        let low = Self::parse_sensitivity(sens_parts[0], s)?;
        let high = if sens_parts.len() > 1 {
            Self::parse_sensitivity(sens_parts[1], s)?
        } else {
            low
        };

        if high < low {
            return Err(ParseError::InvalidSeLinuxRange(s.to_string()));
        }

        // Parse optional categories (c0, c0.c10, c0,c5, etc.)
        let categories = if parts.len() > 1 {
            Some(Self::parse_categories(parts[1], s)?)
        } else {
            None
        };

        Ok(MlsRange { low_sensitivity: low, high_sensitivity: high, categories })
    }

    /// Parses a sensitivity level like "s0" or "s15"
    fn parse_sensitivity(s: &str, full: &str) -> Result<u8, ParseError> {
        if !s.starts_with('s') {
            return Err(ParseError::InvalidSeLinuxRange(full.to_string()));
        }
        s[1..]
            .parse::<u8>()
            .map_err(|_| ParseError::InvalidSeLinuxRange(full.to_string()))
    }

    /// Parses category specifications: "c0", "c0,c5", "c0.c10"
    /// Ranges like "c0.c10" expand to all categories between them
    fn parse_categories(s: &str, full: &str) -> Result<Vec<u16>, ParseError> {
        let mut cats = Vec::new();
        for part in s.split(',') {
            if part.contains('.') {
                // Category range: c0.c10 expands to [0,1,2,...,10]
                let range_parts: Vec<&str> = part.split('.').collect();
                let low = Self::parse_category(range_parts[0], full)?;
                let high = Self::parse_category(range_parts[1], full)?;
                if high < low {
                    return Err(ParseError::InvalidSeLinuxRange(full.to_string()));
                }
                cats.extend(low..=high);
            } else {
                // Single category: c5
                cats.push(Self::parse_category(part, full)?);
            }
        }
        Ok(cats)
    }

    /// Parses a single category like "c0" or "c1023"
    /// Valid range: 0-1023
    fn parse_category(s: &str, full: &str) -> Result<u16, ParseError> {
        if !s.starts_with('c') {
            return Err(ParseError::InvalidSeLinuxRange(full.to_string()));
        }
        let val = s[1..]
            .parse::<u16>()
            .map_err(|_| ParseError::InvalidSeLinuxRange(full.to_string()))?;
        if val > 1023 {
            return Err(ParseError::InvalidSeLinuxRange(full.to_string()));
        }
        Ok(val)
    }

    #[must_use]
    pub fn low_sensitivity(&self) -> u8 {
        self.low_sensitivity
    }
    #[must_use]
    pub fn high_sensitivity(&self) -> u8 {
        self.high_sensitivity
    }
    #[must_use]
    pub fn categories(&self) -> Option<&[u16]> {
        self.categories.as_deref()
    }
}

impl std::fmt::Display for MlsRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "s{}", self.low_sensitivity)?;
        if self.low_sensitivity != self.high_sensitivity {
            write!(f, "-s{}", self.high_sensitivity)?;
        }

        if let Some(cats) = &self.categories {
            write!(f, ":")?;
            let mut i = 0;
            while i < cats.len() {
                if i > 0 {
                    write!(f, ",")?;
                }

                let low = cats[i];
                let low_index = i;
                let mut high = cats[i];
                i += 1;

                while i < cats.len()
                    && cats[i] == low + u16::try_from(i - low_index).unwrap_or(u16::MAX)
                {
                    high += 1;
                    i += 1;
                }

                if low == high {
                    write!(f, "c{low}")?;
                } else {
                    write!(f, "c{low}.c{high}")?;
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selinux_context_new() {
        let ctx = SeLinuxContext::new(Some("user_u".to_string()), "type_t".to_string(), None);

        assert_eq!(ctx.user(), Some("user_u"));
        assert_eq!(ctx.role(), None);
        assert_eq!(ctx.type_(), "type_t");
        assert!(ctx.range().is_none());
    }

    #[test]
    fn selinux_context_with_range() {
        let range = MlsRange::parse("s0").unwrap();
        let ctx = SeLinuxContext::new(None, "type_t".to_string(), Some(range));

        assert!(ctx.user().is_none());
        assert!(ctx.range().is_some());
    }

    #[test]
    fn mls_range_single_sensitivity() {
        let range = MlsRange::parse("s0").unwrap();
        assert_eq!(range.low_sensitivity(), 0);
        assert_eq!(range.high_sensitivity(), 0);
        assert!(range.categories().is_none());
    }

    #[test]
    fn mls_range_sensitivity_range() {
        let range = MlsRange::parse("s0-s15").unwrap();
        assert_eq!(range.low_sensitivity(), 0);
        assert_eq!(range.high_sensitivity(), 15);
    }

    #[test]
    fn mls_range_with_single_category() {
        let range = MlsRange::parse("s0:c0").unwrap();
        assert_eq!(range.categories(), Some(&[0][..]));
    }

    #[test]
    fn mls_range_with_multiple_categories() {
        let range = MlsRange::parse("s0:c0,c5").unwrap();
        assert_eq!(range.categories(), Some(&[0, 5][..]));
    }

    #[test]
    fn mls_range_with_category_range() {
        let range = MlsRange::parse("s0:c0.c3").unwrap();
        assert_eq!(range.categories(), Some(&[0, 1, 2, 3][..]));
    }

    #[test]
    fn mls_range_complex() {
        let range = MlsRange::parse("s0-s1:c0.c5,c10").unwrap();
        assert_eq!(range.low_sensitivity(), 0);
        assert_eq!(range.high_sensitivity(), 1);
        assert_eq!(range.categories(), Some(&[0, 1, 2, 3, 4, 5, 10][..]));
    }

    #[test]
    fn mls_range_invalid_sensitivity() {
        assert!(MlsRange::parse("x0").is_err());
        assert!(MlsRange::parse("s").is_err());
        assert!(MlsRange::parse("sabc").is_err());
    }

    #[test]
    fn mls_range_invalid_range() {
        assert!(MlsRange::parse("s15-s0").is_err());
    }

    #[test]
    fn mls_range_invalid_category() {
        assert!(MlsRange::parse("s0:x0").is_err());
        assert!(MlsRange::parse("s0:c").is_err());
        assert!(MlsRange::parse("s0:c1024").is_err());
    }

    #[test]
    fn mls_range_invalid_category_range() {
        assert!(MlsRange::parse("s0:c10.c0").is_err());
    }

    #[test]
    fn mls_range_display_single() {
        let range = MlsRange::parse("s0").unwrap();
        assert_eq!(range.to_string(), "s0");
    }

    #[test]
    fn mls_range_display_range() {
        let range = MlsRange::parse("s0-s15").unwrap();
        assert_eq!(range.to_string(), "s0-s15");
    }

    #[test]
    fn mls_range_display_with_categories() {
        let range = MlsRange::parse("s0:c0,c5").unwrap();
        assert_eq!(range.to_string(), "s0:c0,c5");
    }

    #[test]
    fn mls_range_display_complex() {
        let range = MlsRange::parse("s0-s1:c0.c2,c5").unwrap();
        assert_eq!(range.to_string(), "s0-s1:c0.c2,c5");
    }
}
