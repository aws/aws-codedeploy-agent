//! `AppSpec` YAML parser — deserializes appspec.yml into typed structs.
use crate::application_specification::{
    Acl, FileExistsBehavior, FileMapping, Files, Hooks, MlsRange, Mode, ObjectType, Os, ParseError,
    Permission, Permissions, ScriptInfo, ScriptLocation, SeLinuxContext, Timeout, Username,
    Version, pattern, types,
};
use indexmap::IndexMap;
use serde::Deserialize;

/// Deserialize hooks map, treating null values as empty vec.
fn deserialize_hooks<'de, D>(deserializer: D) -> Result<IndexMap<String, Vec<RawScript>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: IndexMap<String, Option<Vec<RawScript>>> = IndexMap::deserialize(deserializer)?;
    Ok(raw.into_iter().map(|(k, v)| (k, v.unwrap_or_default())).collect())
}

#[derive(Deserialize)]
pub(crate) struct RawAppSpec {
    version: f64,
    os: String,
    #[serde(default, deserialize_with = "deserialize_hooks")]
    hooks: IndexMap<String, Vec<RawScript>>,
    #[serde(default)]
    files: Vec<RawFileMapping>,
    #[serde(default)]
    permissions: Vec<RawPermission>,
    file_exists_behavior: Option<String>,
}

impl RawAppSpec {
    pub(super) fn validate(self) -> Result<AppSpec, ParseError> {
        let version = Version::parse(self.version)?;
        let os = Os::parse(&self.os)?;

        let file_exists_behavior = match self.file_exists_behavior {
            Some(s) => Some(FileExistsBehavior::parse(&s)?),
            None => None,
        };

        let mut hooks_map = IndexMap::new();
        for (event, scripts) in self.hooks {
            if !scripts.is_empty() {
                let validated: Result<Vec<_>, _> =
                    scripts.into_iter().map(RawScript::validate).collect();
                hooks_map.insert(event, validated?);
            }
        }
        let hooks = Hooks::new(hooks_map);

        let files_vec: Result<Vec<_>, _> =
            self.files.into_iter().map(RawFileMapping::validate).collect();
        let files = Files::new(files_vec?);

        if os == Os::Windows && !self.permissions.is_empty() {
            return Err(ParseError::PermissionsOnWindows);
        }

        let perms_vec: Result<Vec<_>, _> =
            self.permissions.into_iter().map(|p| p.validate(os)).collect();
        let permissions = Permissions::new(perms_vec?);

        Ok(AppSpec { version, os, hooks, files, permissions, file_exists_behavior })
    }
}

#[derive(Deserialize)]
pub(crate) struct RawScript {
    location: Option<String>,
    runas: Option<String>,
    sudo: Option<bool>,
    timeout: Option<u32>,
}

impl RawScript {
    pub(super) fn validate(self) -> Result<ScriptInfo, ParseError> {
        let location = match self.location {
            Some(loc) => ScriptLocation::new(&loc)?,
            None => return Err(ParseError::EmptyScriptLocation),
        };

        let runas = self.runas.and_then(|s| {
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() {
                None
            } else {
                Some(Username(trimmed))
            }
        });

        let timeout = match self.timeout {
            Some(t) => Timeout::new(t)?,
            None => Timeout::default(),
        };

        Ok(ScriptInfo::new(location, runas, self.sudo, timeout))
    }
}

#[derive(Deserialize)]
pub(crate) struct RawFileMapping {
    source: Option<String>,
    destination: Option<String>,
}

impl RawFileMapping {
    pub(super) fn validate(self) -> Result<FileMapping, ParseError> {
        let source = self.source.filter(|s| !s.is_empty()).ok_or(ParseError::MissingSource)?;
        let destination = self
            .destination
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ParseError::MissingDestination(source.clone()))?;
        FileMapping::new(source, destination)
    }
}

#[derive(Deserialize)]
pub(crate) struct RawPermission {
    object: Option<String>,
    pattern: Option<String>,
    except: Option<Vec<String>>,
    #[serde(rename = "type")]
    types: Option<Vec<String>>,
    owner: Option<String>,
    group: Option<String>,
    mode: Option<String>,
    acls: Option<Vec<String>>,
    context: Option<RawContext>,
}

impl RawPermission {
    pub(super) fn validate(self, _os: Os) -> Result<Permission, ParseError> {
        let object = self.object.ok_or(ParseError::MissingPermissionObject)?.trim().to_string();

        if object.is_empty() {
            return Err(ParseError::MissingPermissionObject);
        }

        let pattern = match self.pattern.as_deref() {
            Some("**") | None => pattern::GlobPattern::MatchAll,
            Some(p) => pattern::GlobPattern::compile(p),
        };

        let except: Vec<_> = self
            .except
            .unwrap_or_default()
            .into_iter()
            .map(|p| pattern::GlobPattern::compile(&p))
            .collect();

        let types = match self.types {
            Some(t) => {
                let validated: Result<Vec<_>, _> = t.iter().map(|s| ObjectType::parse(s)).collect();
                validated?
            },
            None => vec![ObjectType::File, ObjectType::Directory],
        };

        let mode = match self.mode {
            Some(m) => Some(Mode::from_octal(&m)?),
            None => None,
        };

        let acls = match self.acls {
            Some(entries) => Some(Acl::parse(&entries)?),
            None => None,
        };

        let context = match self.context {
            Some(ctx) => Some(ctx.validate()?),
            None => None,
        };

        Ok(Permission::new(
            object, pattern, &except, &types, self.owner, self.group, mode, acls, context,
        ))
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct RawContext {
    name: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    range: Option<String>,
}

impl RawContext {
    pub(super) fn validate(self) -> Result<SeLinuxContext, ParseError> {
        let type_ = self.type_.clone().ok_or_else(|| {
            ParseError::InvalidContextType(format!(
                "SELinux context missing required 'type' field: {self:?}"
            ))
        })?;

        let range = match self.range {
            Some(r) => Some(MlsRange::parse(&r)?),
            None => None,
        };

        Ok(SeLinuxContext::new(self.name, type_, range))
    }
}

pub(crate) use types::AppSpec;

#[cfg(test)]
mod tests {
    use super::*;

    // branch_coverage
    #[test]
    fn hooks_all_lifecycle_events() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStop:\n    - location: scripts/stop.sh\n  BeforeInstall:\n    - location: scripts/before_install.sh\n  AfterInstall:\n    - location: scripts/after_install.sh\n  ApplicationStart:\n    - location: scripts/start.sh\n  ValidateService:\n    - location: scripts/validate.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let hooks = spec.hooks();
        assert!(!hooks.get("ApplicationStop").is_empty());
        assert!(!hooks.get("BeforeInstall").is_empty());
        assert!(!hooks.get("AfterInstall").is_empty());
        assert!(!hooks.get("ApplicationStart").is_empty());
        assert!(!hooks.get("ValidateService").is_empty());
    }

    #[test]
    fn script_with_timeout_and_runas() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: scripts/start.sh\n      timeout: 300\n      runas: root\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].timeout(), 300);
        assert_eq!(scripts[0].runas(), Some("root"));
    }

    #[test]
    fn files_with_multiple_mappings() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src1\n    destination: /dest1\n  - source: /src2\n    destination: /dest2\n  - source: /src3\n    destination: /dest3\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.files().iter().count(), 3);
    }

    #[test]
    fn permissions_basic() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp/test\n    owner: root\n    group: root\n    mode: 0755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn empty_hooks() {
        let yaml = "version: 0.0\nos: linux\nhooks: {}\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.hooks().events().count(), 0);
    }

    #[test]
    fn empty_files() {
        let yaml = "version: 0.0\nos: linux\nfiles: []\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.files().iter().count(), 0);
    }

    #[test]
    fn empty_permissions() {
        let yaml = "version: 0.0\nos: linux\npermissions: []\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 0);
    }

    #[test]
    fn os_windows() {
        let yaml = "version: 0.0\nos: windows\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.os(), Os::Windows);
    }

    #[test]
    fn file_exists_behavior_all_variants() {
        let yaml1 = "version: 0.0\nos: linux\nfile_exists_behavior: DISALLOW\n";
        let spec1 = AppSpec::parse(yaml1).unwrap();
        assert_eq!(spec1.file_exists_behavior(), Some(FileExistsBehavior::Disallow));

        let yaml2 = "version: 0.0\nos: linux\nfile_exists_behavior: OVERWRITE\n";
        let spec2 = AppSpec::parse(yaml2).unwrap();
        assert_eq!(spec2.file_exists_behavior(), Some(FileExistsBehavior::Overwrite));

        let yaml3 = "version: 0.0\nos: linux\nfile_exists_behavior: RETAIN\n";
        let spec3 = AppSpec::parse(yaml3).unwrap();
        assert_eq!(spec3.file_exists_behavior(), Some(FileExistsBehavior::Retain));
    }

    // additional_coverage
    #[test]
    fn error_from_io() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let parse_err: ParseError = err.into();
        assert!(matches!(parse_err, ParseError::IoError(_)));
    }

    #[test]
    fn appspec_from_file_not_found() {
        let result = AppSpec::from_file("/nonexistent/file.yaml");
        assert!(matches!(result, Err(ParseError::IoError(_))));
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn version_as_f64() {
        let yaml = "version: 0.0\nos: linux\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.version().as_f64(), 0.0);
    }

    #[test]
    fn os_as_str() {
        assert_eq!(Os::Linux.as_str(), "linux");
        assert_eq!(Os::Windows.as_str(), "windows");
    }

    #[test]
    fn hooks_has_event() {
        let yaml =
            "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert!(spec.hooks().has_event("ApplicationStart"));
        assert!(!spec.hooks().has_event("NonExistent"));
    }

    #[test]
    fn hooks_events_iterator() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n  ApplicationStop:\n    - location: stop.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let events: Vec<&str> = spec.hooks().events().collect();
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn script_sudo() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      sudo: true\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].sudo(), Some(true));
    }

    #[test]
    fn files_source_destination() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: /dest\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.files().iter();
        let mapping = iter.next().unwrap();
        assert_eq!(mapping.source(), "/src");
        assert_eq!(mapping.destination(), "/dest");
    }

    #[test]
    fn permission_accessors() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    owner: root\n    group: wheel\n    mode: 0755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.object(), "/tmp");
        assert_eq!(perm.owner(), Some("root"));
        assert_eq!(perm.group(), Some("wheel"));
        assert!(perm.mode().is_some());
    }

    #[test]
    fn permission_with_acls() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user1:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.acls().is_some());
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn permission_with_context() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.context().is_some());
        assert_eq!(perm.context().unwrap().type_(), "object_t");
    }

    #[test]
    fn selinux_context_accessors() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      name: user_u\n      type: object_t\n      range: s0:c0.c1023\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let ctx = perm.context().unwrap();
        assert_eq!(ctx.user(), Some("user_u"));
        assert_eq!(ctx.type_(), "object_t");
        assert!(ctx.range().is_some());
    }

    #[test]
    fn mls_range_accessors() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s1-s2:c0,c5\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        assert_eq!(range.low_sensitivity(), 1);
        assert_eq!(range.high_sensitivity(), 2);
        assert!(range.categories().is_some());
    }

    #[test]
    fn acl_group_entry() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"g:group1:rw-\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn acl_mask_entry() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"m::r--\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn acl_other_entry() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"o::r--\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn mode_contains() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::OWNER_READ));
        assert!(mode.contains(Mode::OWNER_WRITE));
        assert!(mode.contains(Mode::OWNER_EXECUTE));
    }

    #[test]
    fn mode_setuid_setgid_sticky() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 7777\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::SETUID));
        assert!(mode.contains(Mode::SETGID));
        assert!(mode.contains(Mode::STICKY));
    }

    #[test]
    fn selinux_role() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let ctx = perm.context().unwrap();
        assert_eq!(ctx.role(), None);
    }

    #[test]
    fn selinux_range_with_categories() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c0,c5.c10\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        assert!(range.categories().is_some());
        let cats = range.categories().unwrap();
        assert!(cats.contains(&0));
        assert!(cats.contains(&5));
        assert!(cats.contains(&10));
    }

    #[test]
    fn acl_default_user() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:user:webapp:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn acl_default_group() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:group:webapp:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn acl_default_mask() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:mask::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn acl_default_other() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:other::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 1);
    }

    #[test]
    fn mode_single_digit() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 7\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.mode().is_some());
    }

    #[test]
    fn mode_two_digits() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 55\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.mode().is_some());
    }

    #[test]
    fn mode_three_digits() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.mode().is_some());
    }

    #[test]
    fn hooks_get_nonexistent() {
        let yaml =
            "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.hooks().get("NonExistent").len(), 0);
    }

    #[test]
    fn script_default_timeout() {
        let yaml =
            "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].timeout(), 3600);
    }

    #[test]
    fn permission_no_mode() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    owner: root\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.mode().is_none());
    }

    #[test]
    fn permission_no_acls() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    owner: root\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.acls().is_none());
    }

    #[test]
    fn permission_no_context() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    owner: root\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.context().is_none());
    }

    #[test]
    fn selinux_no_range() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let ctx = perm.context().unwrap();
        assert!(ctx.range().is_none());
    }

    #[test]
    fn selinux_no_user() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let ctx = perm.context().unwrap();
        assert_eq!(ctx.user(), None);
    }

    #[test]
    fn file_exists_behavior_none() {
        let yaml = "version: 0.0\nos: linux\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.file_exists_behavior(), None);
    }

    #[test]
    fn script_no_runas() {
        let yaml =
            "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].runas(), None);
    }

    #[test]
    fn script_no_sudo() {
        let yaml =
            "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].sudo(), None);
    }

    #[test]
    fn selinux_range_high_less_than_low() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s5-s2\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn selinux_category_range_high_less_than_low() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c10.c5\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn selinux_category_over_1023() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c1024\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn selinux_invalid_sensitivity_format() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: x0\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn selinux_invalid_category_format() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:x0\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn selinux_invalid_sensitivity_number() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: sabc\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn selinux_invalid_category_number() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:cabc\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn acl_invalid_length_2() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:name\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn acl_invalid_length_5() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"d:u:name:rwx:extra\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn acl_invalid_type() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"x:name:rwx\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn acl_mask_with_name() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"m:name:rwx\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn acl_other_with_name() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"o:name:rwx\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn mode_zero_padding() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0644\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::OWNER_READ));
        assert!(mode.contains(Mode::OWNER_WRITE));
    }

    #[test]
    fn permission_with_directory_type() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"**/*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn permission_with_both_types() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - file\n      - directory\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn glob_pattern_match() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*.txt\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn glob_pattern_with_except() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"**/*\"\n    except:\n      - \"*.log\"\n      - \"*.tmp\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn hooks_multiple_scripts_per_event() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: script1.sh\n    - location: script2.sh\n      timeout: 300\n    - location: script3.sh\n      runas: root\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts.len(), 3);
    }

    #[test]
    fn files_empty_source() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: \"\"\n    destination: /dest\n";
        let result = AppSpec::parse(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn files_empty_destination() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: \"\"\n";
        let result = AppSpec::parse(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn permission_empty_object() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: \"\"\n    mode: 0755\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::MissingPermissionObject)));
    }

    #[test]
    fn selinux_range_single_sensitivity() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s5\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        assert_eq!(range.low_sensitivity(), 5);
        assert_eq!(range.high_sensitivity(), 5);
    }

    #[test]
    fn selinux_range_with_single_category() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c5\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        let cats = range.categories().unwrap();
        assert_eq!(cats.len(), 1);
        assert_eq!(cats[0], 5);
    }

    #[test]
    fn acl_permissions_all_combinations() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user1:rwx\"\n      - \"u:user2:rw-\"\n      - \"u:user3:r--\"\n      - \"u:user4:---\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.acls().unwrap().entries().len(), 4);
    }

    #[test]
    fn mode_all_zeros() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0000\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert!(perm.mode().is_some());
    }

    #[test]
    fn mode_max_value() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 7777\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::SETUID));
        assert!(mode.contains(Mode::SETGID));
        assert!(mode.contains(Mode::STICKY));
    }

    #[test]
    fn selinux_range_sensitivity_only() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0-s1\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        assert_eq!(range.low_sensitivity(), 0);
        assert_eq!(range.high_sensitivity(), 1);
        assert!(range.categories().is_none());
    }

    #[test]
    fn selinux_range_with_multiple_categories() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c0,c1,c2\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        let cats = range.categories().unwrap();
        assert_eq!(cats.len(), 3);
    }

    #[test]
    fn selinux_range_category_range() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c0.c5\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        let cats = range.categories().unwrap();
        assert_eq!(cats.len(), 6);
    }

    #[test]
    fn selinux_range_mixed_categories() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c0,c5.c7,c10\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        let cats = range.categories().unwrap();
        assert!(cats.contains(&0));
        assert!(cats.contains(&5));
        assert!(cats.contains(&6));
        assert!(cats.contains(&7));
        assert!(cats.contains(&10));
    }

    #[test]
    fn mode_group_bits() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0070\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::GROUP_READ));
        assert!(mode.contains(Mode::GROUP_WRITE));
        assert!(mode.contains(Mode::GROUP_EXECUTE));
    }

    #[test]
    fn mode_world_bits() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0007\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::WORLD_READ));
        assert!(mode.contains(Mode::WORLD_WRITE));
        assert!(mode.contains(Mode::WORLD_EXECUTE));
    }

    #[test]
    fn mode_setgid_only() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 2000\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::SETGID));
        assert!(!mode.contains(Mode::SETUID));
        assert!(!mode.contains(Mode::STICKY));
    }

    #[test]
    fn mode_sticky_only() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 1000\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::STICKY));
        assert!(!mode.contains(Mode::SETUID));
        assert!(!mode.contains(Mode::SETGID));
    }

    #[test]
    fn hooks_event_names() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  BeforeBlockTraffic:\n    - location: before_block.sh\n  AfterBlockTraffic:\n    - location: after_block.sh\n  BeforeAllowTraffic:\n    - location: before_allow.sh\n  AfterAllowTraffic:\n    - location: after_allow.sh\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let hooks = spec.hooks();
        assert!(!hooks.get("BeforeBlockTraffic").is_empty());
        assert!(!hooks.get("AfterBlockTraffic").is_empty());
        assert!(!hooks.get("BeforeAllowTraffic").is_empty());
        assert!(!hooks.get("AfterAllowTraffic").is_empty());
    }

    #[test]
    fn permission_no_owner_no_group() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.owner(), None);
        assert_eq!(perm.group(), None);
    }

    #[test]
    fn script_runas_empty_string() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      runas: \"\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].runas(), None);
    }

    #[test]
    fn script_runas_whitespace_only() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      runas: \"   \"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].runas(), None);
    }

    #[test]
    fn script_runas_with_whitespace() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      runas: \"  root  \"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].runas(), Some("root"));
    }

    #[test]
    fn hooks_empty_script_list() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart: []\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.hooks().get("ApplicationStart").len(), 0);
    }

    #[test]
    fn script_location_none() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - timeout: 300\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::EmptyScriptLocation)));
    }

    #[test]
    fn script_timeout_zero() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      timeout: 0\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidTimeout)));
    }

    #[test]
    fn script_timeout_custom() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      timeout: 600\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].timeout(), 600);
    }

    #[test]
    fn script_sudo_false() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: start.sh\n      sudo: false\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("ApplicationStart");
        assert_eq!(scripts[0].sudo(), Some(false));
    }

    #[test]
    fn files_empty_list() {
        let yaml = "version: 0.0\nos: linux\nfiles: []\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.files().iter().count(), 0);
    }

    #[test]
    fn permissions_empty_list() {
        let yaml = "version: 0.0\nos: linux\npermissions: []\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 0);
    }

    #[test]
    fn file_exists_behavior_none_whitespace() {
        let yaml = "version: 0.0\nos: linux\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.file_exists_behavior(), None);
    }

    #[test]
    fn permission_owner_and_group() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    owner: testuser\n    group: testgroup\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        assert_eq!(perm.owner(), Some("testuser"));
        assert_eq!(perm.group(), Some("testgroup"));
    }

    #[test]
    fn selinux_context_with_name() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      name: system_u\n      type: object_t\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let ctx = perm.context().unwrap();
        assert_eq!(ctx.user(), Some("system_u"));
        assert_eq!(ctx.type_(), "object_t");
    }
}
