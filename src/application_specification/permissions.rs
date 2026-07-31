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
    ) -> Self {
        // File-specific strictness (pattern/except, default-ACL) is enforced at
        // APPLY time via `validate_file_permission`, not here: validation only
        // happens once a permission resolves to an actual copied file. A
        // `pattern:`/`except:` on a directory `object:` with `type: [file]` is
        // valid and must parse, so construction is infallible.
        Permission {
            object,
            pattern,
            except: except.to_vec(),
            types: types.to_vec(),
            owner,
            group,
            mode,
            acls,
            context,
        }
    }

    /// Apply-time validator for file-type permissions.
    ///
    /// Rejects a permission whose `type:` includes `file` if it also declares a
    /// non-`**` `pattern:` or a non-empty `except:` — a combination that only
    /// makes sense for directories.
    ///
    /// Invoked ONLY on the copying-file path, where `object:` directly names a
    /// copied file (`core.rs::process_permission`). It is deliberately NOT called
    /// from the directory-object path (`builder.rs::find_matches`), where
    /// `pattern:`/`except:` legitimately filter the files under a directory
    /// `object:` and only the ACL is validated per match. Validation is at apply
    /// time, not parse time, for the same reason.
    ///
    /// # Errors
    /// Returns [`ParseError::InvalidFilePattern`] when `pattern` is anything other than
    /// `**`, or [`ParseError::InvalidFileExcept`] when an `except` list is set on a
    /// file permission.
    pub fn validate_file_permission(&self) -> Result<(), ParseError> {
        if !self.types.contains(&ObjectType::File) {
            return Ok(());
        }

        if !matches!(self.pattern, pattern::GlobPattern::MatchAll) {
            return Err(ParseError::InvalidFilePattern(
                self.object.clone(),
                self.pattern.as_str().to_string(),
            ));
        }

        if !self.except.is_empty() {
            return Err(ParseError::InvalidFileExcept(
                self.except.iter().map(|p| p.as_str().to_string()).collect(),
                self.object.clone(),
            ));
        }

        Ok(())
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

    /// Apply-time ACL validator for file targets.
    ///
    /// Rejects a permission that carries default ACL entries when applied to a
    /// file, since default ACLs are only meaningful on directories.
    ///
    /// # Errors
    /// Returns [`ParseError::DefaultAclOnFile`] if the permission declares any
    /// default ACL entry.
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
        );

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
            &[pattern::GlobPattern::compile("*.log")],
            &[ObjectType::Directory],
            None,
            None,
            None,
            None,
            None,
        );

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

    // File-typed permissions parse even with directory-shaped `pattern:` /
    // `except:`; enforcement is deferred to apply time.
    #[test]
    fn permission_file_pattern_parses_at_parse_time() {
        // `type: [file]` with a non-`**` pattern parses cleanly; validation is
        // deferred to the installer.
        let perm = Permission::new(
            "/app/file.txt".to_string(),
            pattern::GlobPattern::compile("*.txt"),
            &[],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );
        // Construction is infallible; the file-strictness check is at apply time.
        assert_eq!(perm.object(), "/app/file.txt");
    }

    #[test]
    fn permission_file_pattern_rejected_at_apply_time() {
        // A file-typed permission with a non-`**` pattern is rejected by
        // `validate_file_permission` (apply time).
        let perm = Permission::new(
            "/app/file.txt".to_string(),
            pattern::GlobPattern::compile("*.txt"),
            &[],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );

        match perm.validate_file_permission() {
            Err(ParseError::InvalidFilePattern(object, pat)) => {
                assert_eq!(object, "/app/file.txt");
                // The user's original glob shows up in the error — not the
                // globset matcher's `Debug` output.
                assert_eq!(pat, "*.txt");
            },
            other => panic!("expected InvalidFilePattern, got {other:?}"),
        }
    }

    #[test]
    fn permission_file_underscore_wildcard_parses_at_parse_time() {
        // A directory `object:` with `pattern: file_*` + `type: [file]` (files
        // under it selected by the glob) is valid input and must parse.
        let perm = Permission::new(
            "/agent_test".to_string(),
            pattern::GlobPattern::compile("file_*"),
            &[pattern::GlobPattern::compile("file_755")],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(perm.object(), "/agent_test");
    }

    #[test]
    fn permission_file_except_parses_at_parse_time() {
        let perm = Permission::new(
            "/app/file.txt".to_string(),
            pattern::GlobPattern::MatchAll,
            &[pattern::GlobPattern::compile("*.log")],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );
        assert_eq!(perm.object(), "/app/file.txt");
    }

    #[test]
    fn permission_file_except_rejected_at_apply_time() {
        let perm = Permission::new(
            "/app/file.txt".to_string(),
            pattern::GlobPattern::MatchAll,
            &[pattern::GlobPattern::compile("*.log")],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );

        match perm.validate_file_permission() {
            Err(ParseError::InvalidFileExcept(excepts, object)) => {
                assert_eq!(object, "/app/file.txt");
                assert_eq!(excepts, vec!["*.log".to_string()]);
            },
            other => panic!("expected InvalidFileExcept, got {other:?}"),
        }
    }

    #[test]
    fn permission_directory_pattern_never_calls_file_check() {
        // A directory-only permission with a glob pattern is legitimate and
        // must never fire `validate_file_permission` (no `type: file`).
        let perm = Permission::new(
            "/app".to_string(),
            pattern::GlobPattern::compile("*.log"),
            &[pattern::GlobPattern::compile("audit_*")],
            &[ObjectType::Directory],
            None,
            None,
            None,
            None,
            None,
        );
        assert!(perm.validate_file_permission().is_ok());
    }

    #[test]
    fn invalid_file_pattern_error_carries_user_glob_not_debug_output() {
        // The error must contain the user's glob string, not the compiled
        // matcher's `Debug` output (which dumps regex-automata internals).
        let perm = Permission::new(
            "/agent_test".to_string(),
            pattern::GlobPattern::compile("file_*"),
            &[],
            &[ObjectType::File],
            None,
            None,
            None,
            None,
            None,
        );

        let err = perm.validate_file_permission().unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("file_*"), "user glob missing from error: {msg}");
        assert!(!msg.contains("GlobMatcher"), "matcher Debug leaked into error: {msg}");
        assert!(!msg.contains("Regex"), "regex Debug leaked into error: {msg}");
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
        );

        assert!(perm.context().is_some());
    }

    #[test]
    fn default_acl_on_file_error() {
        // A file-typed permission declaring a default ACL parses, but
        // `validate_file_acl` rejects it at apply time.
        let acl = Acl::parse(&["default:user:testuser:rwx".to_string()]).unwrap();

        let perm = Permission::new(
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

        match perm.validate_file_acl(Path::new("/app/file.txt")) {
            Err(ParseError::DefaultAclOnFile) => {},
            other => panic!("Expected DefaultAclOnFile at apply time, got {other:?}"),
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
        );

        // validate_file_acl should still catch default ACLs
        assert!(perm.validate_file_acl(Path::new("/app/file.txt")).is_err());
    }

    #[test]
    fn matches_except_with_trailing_slash_object() {
        let perm = Permission::new(
            "/app/".to_string(),
            pattern::GlobPattern::MatchAll,
            &[pattern::GlobPattern::compile("*.log")],
            &[ObjectType::Directory],
            None,
            None,
            None,
            None,
            None,
        );

        assert!(perm.matches_except(Path::new("/app/file.log")));
        assert!(!perm.matches_except(Path::new("/app/file.txt")));
        // Path outside the base object — early return false
        assert!(!perm.matches_except(Path::new("/other/file.log")));
    }
}
