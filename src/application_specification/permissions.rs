//! @risk medium
//!
//! `AppSpec` file permission types.
use crate::application_specification::{Acl, Mode, ParseError, SeLinuxContext, pattern};

#[derive(Debug, Clone, PartialEq)]
pub struct Permission {
    object: String,
    pattern: pattern::GlobPattern,
    except: Vec<pattern::GlobPattern>,
    types: Vec<ObjectType>,
    owner: Option<String>,
    group: Option<String>,
    mode: Option<Mode>,
    acls: Option<Acl>,
    context: Option<SeLinuxContext>,
}

impl Permission {
    // Allow many arguments - this is an internal constructor called once from parse.rs
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        object: String,
        pattern: pattern::GlobPattern,
        except: &[pattern::GlobPattern],
        types: &[ObjectType],
        owner: Option<String>,
        group: Option<String>,
        mode: Option<Mode>,
        acls: Option<Acl>,
        context: Option<SeLinuxContext>,
    ) -> Result<Self, ParseError> {
        let perm = Permission {
            object: object.clone(),
            pattern,
            except: except.to_vec(),
            types: types.to_vec(),
            owner,
            group,
            mode,
            acls,
            context,
        };

        // Validate file-specific constraints
        // Note: Permission validation happens during installation, not at parse time.
        // not during parsing. Rust validates earlier (during parse) for fail-fast behavior.
        // Both use the same validation logic, just different timing.
        if types.contains(&ObjectType::File) {
            if !matches!(perm.pattern, pattern::GlobPattern::MatchAll) {
                return Err(ParseError::InvalidFilePattern(
                    object.clone(),
                    format!("{:?}", perm.pattern),
                ));
            }
            if !except.is_empty() {
                return Err(ParseError::InvalidFileExcept(
                    except.iter().map(|p| format!("{p:?}")).collect(),
                    object,
                ));
            }
            // Check for default ACLs on files
            if perm.acls.as_ref().is_some_and(super::acl::Acl::has_default_entries) {
                return Err(ParseError::DefaultAclOnFile);
            }
        }

        Ok(perm)
    }

    /// Test-only constructor for creating Permission objects in tests
    #[cfg(test)]
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new_for_test(
        object: String,
        types: Vec<ObjectType>,
        except: Vec<String>,
        owner: Option<String>,
        group: Option<String>,
        mode: Option<Mode>,
        acls: Option<Acl>,
        context: Option<SeLinuxContext>,
    ) -> Self {
        Permission {
            object,
            pattern: pattern::GlobPattern::MatchAll,
            except: except.into_iter().map(|s| pattern::GlobPattern::compile(&s)).collect(),
            types,
            owner,
            group,
            mode,
            acls,
            context,
        }
    }

    #[must_use]
    pub fn object(&self) -> &str {
        &self.object
    }
    #[must_use]
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
    #[must_use]
    pub fn group(&self) -> Option<&str> {
        self.group.as_deref()
    }
    #[must_use]
    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }
    #[must_use]
    pub fn acls(&self) -> Option<&Acl> {
        self.acls.as_ref()
    }
    #[must_use]
    pub fn context(&self) -> Option<&SeLinuxContext> {
        self.context.as_ref()
    }

    #[must_use]
    pub fn types(&self) -> &[ObjectType] {
        &self.types
    }

    #[must_use]
    pub fn matches_pattern(&self, path: &std::path::Path) -> bool {
        use std::path::Path;

        let name = path.to_string_lossy();
        let name = name.trim_end_matches('/');

        let base_object = Path::new(&self.object);
        let base_str = base_object.to_string_lossy();
        let base_with_sep = if base_str.ends_with('/') {
            base_str.to_string()
        } else {
            format!("{base_str}/")
        };

        if name.starts_with(&base_with_sep) {
            let rel_name = &name[base_with_sep.len()..];
            return self.pattern.matches(rel_name);
        }

        false
    }

    #[must_use]
    pub fn matches_except(&self, path: &std::path::Path) -> bool {
        use std::path::Path;

        let name = path.to_string_lossy();
        let name = name.trim_end_matches('/');

        let base_object = Path::new(&self.object);
        let base_str = base_object.to_string_lossy();
        let base_with_sep = if base_str.ends_with('/') {
            base_str.to_string()
        } else {
            format!("{base_str}/")
        };

        if !name.starts_with(&base_with_sep) {
            return false;
        }

        let rel_name = &name[base_with_sep.len()..];
        self.except.iter().any(|p| p.matches(rel_name))
    }

    /// # Errors
    /// Returns an error if ACL entries contain default entries for non-directory objects.
    pub fn validate_file_acl(&self, _object: &std::path::Path) -> Result<(), ParseError> {
        if self.acls.as_ref().is_some_and(super::acl::Acl::has_default_entries) {
            return Err(ParseError::DefaultAclOnFile);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Permissions(Vec<Permission>);

impl Permissions {
    pub(crate) fn new(vec: Vec<Permission>) -> Self {
        Permissions(vec)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Permission> {
        self.0.iter()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectType {
    File,
    Directory,
}

impl ObjectType {
    pub(crate) fn parse(s: &str) -> Result<Self, ParseError> {
        match s {
            "file" => Ok(ObjectType::File),
            "directory" => Ok(ObjectType::Directory),
            _ => Err(ParseError::InvalidObjectType(s.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn create_test_permission(object: &str, types: &[ObjectType]) -> Permission {
        Permission::new(
            object.to_string(),
            pattern::GlobPattern::MatchAll,
            &[],
            types,
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[test]
    fn permission_getters() {
        let perm = Permission::new(
            "/app".to_string(),
            pattern::GlobPattern::MatchAll,
            &[],
            &[ObjectType::Directory],
            Some("user".to_string()),
            Some("group".to_string()),
            Some(Mode::from_octal("755").unwrap()),
            None,
            None,
        )
        .unwrap();

        assert_eq!(perm.object(), "/app");
        assert_eq!(perm.owner(), Some("user"));
        assert_eq!(perm.group(), Some("group"));
        assert!(perm.mode().is_some());
        assert_eq!(perm.types(), &[ObjectType::Directory]);
    }

    #[test]
    fn matches_pattern_directory() {
        let perm = create_test_permission("/app", &[ObjectType::Directory]);

        assert!(perm.matches_pattern(Path::new("/app/file.txt")));
        assert!(perm.matches_pattern(Path::new("/app/subdir/file.txt")));
        assert!(!perm.matches_pattern(Path::new("/other/file.txt")));
    }

    #[test]
    fn matches_pattern_with_trailing_slash() {
        let perm = create_test_permission("/app/", &[ObjectType::Directory]);

        assert!(perm.matches_pattern(Path::new("/app/file.txt")));
        assert!(perm.matches_pattern(Path::new("/app/subdir/file.txt")));
    }

    #[test]
    fn matches_except() {
        let perm = Permission::new(
            "/app".to_string(),
            pattern::GlobPattern::MatchAll,
            &[pattern::GlobPattern::Wildcard("*.log".to_string())],
            &[ObjectType::Directory],
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert!(perm.matches_except(Path::new("/app/file.log")));
        assert!(!perm.matches_except(Path::new("/app/file.txt")));
    }

    #[test]
    fn validate_file_acl_no_acl() {
        let perm = create_test_permission("/app/file.txt", &[ObjectType::File]);
        assert!(perm.validate_file_acl(Path::new("/app/file.txt")).is_ok());
    }

    #[test]
    fn object_type_parse() {
        assert_eq!(ObjectType::parse("file").unwrap(), ObjectType::File);
        assert_eq!(ObjectType::parse("directory").unwrap(), ObjectType::Directory);
        assert!(ObjectType::parse("invalid").is_err());
    }

    #[test]
    fn permissions_iter() {
        let perm1 = create_test_permission("/app", &[ObjectType::Directory]);
        let perm2 = create_test_permission("/data", &[ObjectType::Directory]);

        let permission_set = Permissions::new(vec![perm1, perm2]);
        assert_eq!(permission_set.iter().count(), 2);
    }

    #[test]
    fn permission_file_pattern_validation() {
        let result = Permission::new(
            "/app/file.txt".to_string(),
            pattern::GlobPattern::Wildcard("*.txt".to_string()),
            &[],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn permission_file_except_validation() {
        let result = Permission::new(
            "/app/file.txt".to_string(),
            pattern::GlobPattern::MatchAll,
            &[pattern::GlobPattern::Wildcard("*.log".to_string())],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn permission_with_context() {
        let ctx = SeLinuxContext::new(Some("user_u".to_string()), "type_t".to_string(), None);

        let perm = Permission::new(
            "/app".to_string(),
            pattern::GlobPattern::MatchAll,
            &[],
            &[ObjectType::Directory],
            None,
            None,
            None,
            None,
            Some(ctx),
        )
        .unwrap();

        assert!(perm.context().is_some());
    }

    #[test]
    fn default_acl_on_file_error() {
        let acl = Acl::parse(&["default:user:testuser:rwx".to_string()]).unwrap();

        let result = Permission::new(
            "/app".to_string(),
            pattern::GlobPattern::MatchAll,
            &[],
            &[ObjectType::File],
            None,
            None,
            None,
            Some(acl),
            None,
        );

        assert!(result.is_err());
        match result.unwrap_err() {
            ParseError::DefaultAclOnFile => {},
            _ => panic!("Expected DefaultAclOnFile error"),
        }
    }

    #[test]
    fn validate_file_acl_with_default_acl() {
        let acl = Acl::parse(&["default:user:testuser:rwx".to_string()]).unwrap();
        // Create a directory permission (bypasses constructor validation for files)
        let perm = Permission::new(
            "/app".to_string(),
            pattern::GlobPattern::MatchAll,
            &[],
            &[ObjectType::Directory],
            None,
            None,
            None,
            Some(acl),
            None,
        )
        .unwrap();

        // validate_file_acl should still catch default ACLs
        assert!(perm.validate_file_acl(Path::new("/app/file.txt")).is_err());
    }

    #[test]
    fn matches_except_with_trailing_slash_object() {
        let perm = Permission::new(
            "/app/".to_string(),
            pattern::GlobPattern::MatchAll,
            &[pattern::GlobPattern::Wildcard("*.log".to_string())],
            &[ObjectType::Directory],
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert!(perm.matches_except(Path::new("/app/file.log")));
        assert!(!perm.matches_except(Path::new("/app/file.txt")));
        // Path outside the base object — early return false
        assert!(!perm.matches_except(Path::new("/other/file.log")));
    }
}
