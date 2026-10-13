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

    format!("https://{service}.{region}.{}", partition_dns_suffix(region))
}

/// Map a region to its partition's DNS suffix.
///
/// The default AWS partition uses `amazonaws.com`; other partitions have
/// distinct suffixes and are matched by their region-name prefix:
/// - `cn-*` (China) -> `amazonaws.com.cn`
/// - `eusc-*` (European Sovereign Cloud) -> `amazonaws.eu`
/// - `us-iso-*` (ISO) -> `c2s.ic.gov`
/// - `us-isob-*` (ISO-B) -> `sc2s.sgov.gov`
/// - `eu-isoe-*` (ISO-E) -> `cloud.adc-e.uk`
/// - `us-isof-*` (ISO-F) -> `csp.hci.ic.gov`
///
/// `GovCloud` (`us-gov-*`) intentionally stays on `amazonaws.com`.
///
/// NOTE: this prefix table is a stopgap. The legacy Ruby agent read the domain
/// directly from IMDS (`/latest/meta-data/services/domain`), which resolves
/// every partition without hardcoding. Once IMDS domain resolution lands, the
/// non-commercial entries here should be removed. The ISO prefixes are
/// mutually disjoint (`us-iso-`, `us-isob-`, `us-isof-` never overlap), so
/// match order is not significant.
fn partition_dns_suffix(region: &str) -> &'static str {
    if region.starts_with("cn-") {
        "amazonaws.com.cn"
    } else if region.starts_with("eusc-") {
        "amazonaws.eu"
    } else if region.starts_with("us-iso-") {
        "c2s.ic.gov"
    } else if region.starts_with("us-isob-") {
        "sc2s.sgov.gov"
    } else if region.starts_with("eu-isoe-") {
        "cloud.adc-e.uk"
    } else if region.starts_with("us-isof-") {
        "csp.hci.ic.gov"
    } else {
        "amazonaws.com"
    }
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
    fn eusc_region_endpoint() {
        assert_eq!(
            resolve("eusc-de-east-1", None, false, false),
            "https://codedeploy-commands.eusc-de-east-1.amazonaws.eu"
        );
    }

    #[test]
    fn eusc_region_secure_endpoint() {
        assert_eq!(
            resolve("eusc-de-east-1", None, false, true),
            "https://codedeploy-commands-secure.eusc-de-east-1.amazonaws.eu"
        );
    }

    #[test]
    fn gov_cloud_stays_on_default_domain() {
        assert_eq!(
            resolve("us-gov-west-1", None, false, true),
            "https://codedeploy-commands-secure.us-gov-west-1.amazonaws.com"
        );
    }

    #[test]
    fn iso_region_endpoint() {
        assert_eq!(
            resolve("us-iso-east-1", None, false, false),
            "https://codedeploy-commands.us-iso-east-1.c2s.ic.gov"
        );
    }

    #[test]
    fn iso_b_region_endpoint() {
        assert_eq!(
            resolve("us-isob-east-1", None, false, false),
            "https://codedeploy-commands.us-isob-east-1.sc2s.sgov.gov"
        );
    }

    #[test]
    fn iso_e_region_endpoint() {
        assert_eq!(
            resolve("eu-isoe-west-1", None, false, false),
            "https://codedeploy-commands.eu-isoe-west-1.cloud.adc-e.uk"
        );
    }

    #[test]
    fn iso_f_region_endpoint() {
        assert_eq!(
            resolve("us-isof-south-1", None, false, false),
            "https://codedeploy-commands.us-isof-south-1.csp.hci.ic.gov"
        );
    }

    #[test]
    fn iso_region_secure_endpoint() {
        assert_eq!(
            resolve("us-iso-east-1", None, false, true),
            "https://codedeploy-commands-secure.us-iso-east-1.c2s.ic.gov"
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
