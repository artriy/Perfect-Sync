use semver::Version as SemVersion;
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Version {
    SemVer(SemVersion),
    /// BepInEx build feeds also expose bare `be.N` identifiers.
    BepInExBuild(u64),
}

pub fn parse(input: &str) -> Option<Version> {
    let value = input.trim();
    if let Some(build) = value.strip_prefix("be.") {
        if !build.is_empty() && build.bytes().all(|byte| byte.is_ascii_digit()) {
            return build.parse().ok().map(Version::BepInExBuild);
        }
        return None;
    }
    let value = value.strip_prefix('v').unwrap_or(value);
    SemVersion::parse(value).ok().map(Version::SemVer)
}

/// Compare supported version tags. Unsupported or incomparable tag forms do
/// not receive a synthetic ordering.
pub fn cmp(a: &str, b: &str) -> Option<Ordering> {
    match (parse(a)?, parse(b)?) {
        (Version::SemVer(a), Version::SemVer(b)) => Some(a.cmp_precedence(&b)),
        (Version::BepInExBuild(a), Version::BepInExBuild(b)) => Some(a.cmp(&b)),
        _ => None,
    }
}

/// True if `candidate` is a strictly newer supported release than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    cmp(candidate, current) == Some(Ordering::Greater)
}

/// Match a release tag against every validated semver requirement.
pub fn satisfies_all(tag: &str, requirements: &[String]) -> bool {
    let Some(Version::SemVer(version)) = parse(tag) else {
        return false;
    };
    requirements.iter().all(|requirement| {
        semver::VersionReq::parse(requirement.trim())
            .is_ok_and(|requirement| requirement.matches(&version))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert!(is_newer("1.6.3", "1.6.2"));
        assert!(!is_newer("1.6.2", "1.6.3"));
        assert_eq!(cmp("1.6.2", "1.6.2"), Some(Ordering::Equal));
    }

    #[test]
    fn strips_v_prefix() {
        assert_eq!(cmp("v4.8.0", "4.8.0"), Some(Ordering::Equal));
        assert!(is_newer("v4.8.0", "v4.7.2"));
    }

    #[test]
    fn date_based_ordering_is_numeric() {
        assert!(is_newer("2025.11.20", "2025.9.4"));
    }

    #[test]
    fn bepinex_be_builds() {
        assert!(is_newer("6.0.0-be.735", "6.0.0-be.697"));
        assert_eq!(cmp("6.0.0-be.735", "6.0.0-be.735"), Some(Ordering::Equal));
    }

    #[test]
    fn build_metadata_does_not_affect_precedence() {
        assert_eq!(cmp("1.0.0+build.2", "1.0.0+build.1"), Some(Ordering::Equal));
    }

    #[test]
    fn semver_stable_outranks_every_prerelease() {
        assert!(is_newer("6.0.0", "6.0.0-be.764"));
        assert!(is_newer("1.0.0-rc.10", "1.0.0-rc.2"));
        assert!(!is_newer("6.0.0-be.764", "6.0.0"));
    }

    #[test]
    fn bare_be_markers_order_numerically() {
        assert!(is_newer("be.770", "be.764"));
        assert_eq!(cmp("be.764", "be.764"), Some(Ordering::Equal));
    }

    #[test]
    fn matches_combined_dependency_requirements() {
        let requirements = vec![">=2.0.0".into(), "<=2.4.0".into()];
        assert!(satisfies_all("v2.4.0", &requirements));
        assert!(!satisfies_all("2.5.0", &requirements));
        assert!(!satisfies_all("release-next", &requirements));
    }

    #[test]
    fn unsupported_tags_have_no_ordering() {
        assert_eq!(cmp("release-next", "release-old"), None);
        assert_eq!(cmp("1.2", "1.1.9"), None);
        assert!(!is_newer("release-next", "1.0.0"));
    }
}
