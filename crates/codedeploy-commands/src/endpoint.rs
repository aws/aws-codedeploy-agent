//! Endpoint URL construction for the `CodeDeploy` Command Service.

/// Resolve the service endpoint URL.
///
/// Priority: custom override > constructed from region + flags.
/// Appends `-secure` and/or `-fips` suffixes based on config, then
/// `https://{service}.{region}.{domain}`.
#[must_use]
pub fn resolve(
    region: &str,
    endpoint_override: Option<&str>,
    use_fips: bool,
    enable_auth_policy: bool,
) -> String {
    if let Some(url) = endpoint_override {
        return url.to_string();
    }

    let mut service = "codedeploy-commands".to_string();
    if enable_auth_policy {
        service.push_str("-secure");
    }
    if use_fips {
        service.push_str("-fips");
    }

    let domain = if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else {
        "amazonaws.com"
    };

    format!("https://{service}.{region}.{domain}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_endpoint() {
        assert_eq!(
            resolve("us-east-1", None, false, false),
            "https://codedeploy-commands.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn fips_endpoint() {
        assert_eq!(
            resolve("us-west-2", None, true, false),
            "https://codedeploy-commands-fips.us-west-2.amazonaws.com"
        );
    }

    #[test]
    fn auth_policy_endpoint() {
        assert_eq!(
            resolve("us-east-1", None, false, true),
            "https://codedeploy-commands-secure.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn fips_and_auth_policy_endpoint() {
        assert_eq!(
            resolve("us-east-1", None, true, true),
            "https://codedeploy-commands-secure-fips.us-east-1.amazonaws.com"
        );
    }

    #[test]
    fn china_region_endpoint() {
        assert_eq!(
            resolve("cn-north-1", None, false, false),
            "https://codedeploy-commands.cn-north-1.amazonaws.com.cn"
        );
    }

    #[test]
    fn custom_override_bypasses_construction() {
        assert_eq!(
            resolve("us-east-1", Some("https://custom.endpoint"), true, true),
            "https://custom.endpoint"
        );
    }
}
