//! The Windows MSI (`wix/main.wxs`) installs the agent under
//! `%ProgramData%\Amazon\CodeDeploy` and gives that install root an explicit
//! Administrators-only ACL (Windows Installer adds LocalSystem itself) instead
//! of the permissions it would otherwise inherit from `%ProgramData%`.
//!
//! The model is the one the 1.x MSI shipped with: a single Administrators
//! Permission on the install root, which every subfolder inherits. The MSI is
//! not built by this crate, so this test reads the WiX source and fails the
//! build if that ACL is removed, loosened, or extended to other folders.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Directory `Id` of the install root in `wix/main.wxs`.
const INSTALL_ROOT: &str = "INSTALLDIR";
/// The only grantee allowed in the package (LocalSystem is implicit).
const ADMINISTRATORS: &str = "Administrators";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TagKind {
    Open,
    Close,
    SelfClosing,
}

#[derive(Debug)]
struct Tag {
    name: String,
    attrs: BTreeMap<String, String>,
    kind: TagKind,
}

/// Minimal tag scanner for the WiX source: skips comments and processing
/// instructions, and ignores `>` inside quoted attribute values. Enough for
/// `main.wxs`, which uses no CDATA or entities in the elements checked here.
fn scan_tags(xml: &str) -> Vec<Tag> {
    let mut tags = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        rest = &rest[start..];
        let skip_to = |end: &str, rest: &str| rest.find(end).map_or(rest.len(), |i| i + end.len());
        if rest.starts_with("<!--") {
            rest = &rest[skip_to("-->", rest)..];
            continue;
        }
        if rest.starts_with("<?") {
            rest = &rest[skip_to("?>", rest)..];
            continue;
        }
        let mut in_quote = false;
        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                if c == '"' {
                    in_quote = !in_quote;
                }
                c == '>' && !in_quote
            })
            .map_or(rest.len(), |(i, _)| i);
        tags.push(parse_tag(&rest[1..end]));
        rest = &rest[(end + 1).min(rest.len())..];
    }
    tags
}

fn parse_tag(inner: &str) -> Tag {
    let (kind, body) = if let Some(body) = inner.strip_prefix('/') {
        (TagKind::Close, body)
    } else if let Some(body) = inner.strip_suffix('/') {
        (TagKind::SelfClosing, body)
    } else {
        (TagKind::Open, inner)
    };
    let body = body.trim();
    let name_end = body.find(char::is_whitespace).unwrap_or(body.len());
    let mut attrs = BTreeMap::new();
    let mut rest = &body[name_end..];
    while let Some(eq) = rest.find('=') {
        let key = rest[..eq].trim().to_string();
        let after = &rest[eq + 1..];
        let Some(open) = after.find('"') else { break };
        let Some(len) = after[open + 1..].find('"') else {
            break;
        };
        attrs.insert(key, after[open + 1..open + 1 + len].to_string());
        rest = &after[open + 1 + len + 1..];
    }
    Tag { name: body[..name_end].to_string(), attrs, kind }
}

/// What the ACL checks need from the package.
#[derive(Debug, Default)]
struct Package {
    /// Folder Id → (component Id, Permission attributes) for each
    /// `CreateFolder/Permission`.
    folder_permissions: Vec<(String, String, BTreeMap<String, String>)>,
    /// `User` of every `Permission`/`PermissionEx`, wherever it appears.
    grantees: Vec<String>,
    /// Ids referenced by `ComponentRef`.
    component_refs: BTreeSet<String>,
}

fn attr<'a>(tag: &'a Tag, key: &str) -> Option<&'a str> {
    tag.attrs.get(key).map(String::as_str)
}

fn analyze(xml: &str) -> Package {
    let mut pkg = Package::default();
    let mut stack: Vec<Tag> = Vec::new();
    for tag in scan_tags(xml) {
        if tag.kind == TagKind::Close {
            stack.pop();
            continue;
        }
        let nearest = |name: &str| stack.iter().rev().find(|t| t.name == name);
        // WiX resolves a folder from an explicit Directory attribute, then the
        // component's Directory, then the enclosing <Directory>.
        let folder = |own: Option<&str>| {
            own.or_else(|| nearest("Component").and_then(|c| attr(c, "Directory")))
                .or_else(|| nearest("Directory").and_then(|d| attr(d, "Id")))
                .unwrap_or_default()
                .to_string()
        };
        match tag.name.as_str() {
            "Permission" | "PermissionEx" => {
                pkg.grantees.push(attr(&tag, "User").unwrap_or_default().to_string());
                if let Some(create) = stack.last().filter(|t| t.name == "CreateFolder") {
                    let component = nearest("Component")
                        .and_then(|c| attr(c, "Id"))
                        .unwrap_or_default()
                        .to_string();
                    pkg.folder_permissions.push((
                        folder(attr(create, "Directory")),
                        component,
                        tag.attrs.clone(),
                    ));
                }
            },
            "ComponentRef" => {
                pkg.component_refs.insert(attr(&tag, "Id").unwrap_or_default().to_string());
            },
            _ => {},
        }
        if tag.kind == TagKind::Open {
            stack.push(tag);
        }
    }
    pkg
}

fn main_wxs() -> Package {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("wix/main.wxs");
    let xml = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
    analyze(&xml)
}

/// Folders that get an Administrators/GenericAll descriptor from a component
/// the feature actually installs.
fn locked_folders(pkg: &Package) -> BTreeSet<String> {
    pkg.folder_permissions
        .iter()
        .filter(|(_, component, attrs)| {
            pkg.component_refs.contains(component)
                && attrs.get("User").map(String::as_str) == Some(ADMINISTRATORS)
                && attrs.get("GenericAll").map(String::as_str) == Some("yes")
        })
        .map(|(folder, _, _)| folder.clone())
        .collect()
}

#[test]
fn only_install_root_is_locked_to_administrators() {
    // Arrange
    let pkg = main_wxs();

    // Act
    let locked = locked_folders(&pkg);

    // Assert
    assert_eq!(
        locked,
        BTreeSet::from([INSTALL_ROOT.to_string()]),
        "expected exactly one installed CreateFolder/Permission User=\"{ADMINISTRATORS}\" \
         GenericAll=\"yes\", on {INSTALL_ROOT}"
    );
}

#[test]
fn package_declares_no_other_permissions() {
    // Arrange
    let pkg = main_wxs();

    // Act
    let count = pkg.grantees.len();

    // Assert
    assert_eq!(count, 1, "wix/main.wxs must declare exactly one Permission (on {INSTALL_ROOT})");
}

#[test]
fn no_permission_grants_access_beyond_administrators() {
    // Arrange
    let pkg = main_wxs();

    // Act
    let others: Vec<_> = pkg.grantees.iter().filter(|u| *u != ADMINISTRATORS).collect();

    // Assert
    assert!(others.is_empty(), "unexpected grantees in wix/main.wxs: {others:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanner_ignores_tags_inside_comments() {
        let xml = r#"<Wix><!-- <Permission User="Everyone" /> --><Feature Id="F" /></Wix>"#;
        assert!(analyze(xml).grantees.is_empty());
    }

    #[test]
    fn scanner_ignores_angle_bracket_in_attribute_value() {
        let tags = scan_tags(r#"<Product Description="a > b" Id="P" />"#);
        assert_eq!(tags[0].attrs.get("Id").map(String::as_str), Some("P"));
    }

    #[test]
    fn create_folder_directory_attribute_overrides_enclosing_directory() {
        let xml = r#"<Directory Id="A"><Component Id="C"><CreateFolder Directory="B">
                     <Permission User="Administrators" GenericAll="yes" />
                     </CreateFolder></Component></Directory><ComponentRef Id="C" />"#;
        assert_eq!(locked_folders(&analyze(xml)), BTreeSet::from(["B".to_string()]));
    }

    #[test]
    fn permission_in_unreferenced_component_does_not_lock_folder() {
        let xml = r#"<Directory Id="A"><Component Id="C"><CreateFolder>
                     <Permission User="Administrators" GenericAll="yes" />
                     </CreateFolder></Component></Directory>"#;
        assert!(locked_folders(&analyze(xml)).is_empty());
    }
}
