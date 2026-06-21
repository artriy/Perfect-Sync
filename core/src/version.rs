use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    parts: Vec<u64>,
    be: Option<u64>,
}

pub fn parse(s: &str) -> Version {
    let (main, be) = match s.split_once("-be.") {
        Some((m, b)) => (m, b.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse().ok()),
        None => (s, None),
    };
    let main = main.trim_start_matches('v');
    let parts = main
        .split(['.', '-'])
        .filter_map(|p| {
            let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().ok()
        })
        .collect();
    Version { parts, be }
}

pub fn cmp(a: &str, b: &str) -> Ordering {
    let (va, vb) = (parse(a), parse(b));
    va.parts.cmp(&vb.parts).then_with(|| match (va.be, vb.be) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    })
}

/// True if `candidate` is a strictly newer release than `current`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    cmp(candidate, current) == Ordering::Greater
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_ordering() {
        assert!(is_newer("1.6.3", "1.6.2"));
        assert!(!is_newer("1.6.2", "1.6.3"));
        assert_eq!(cmp("1.6.2", "1.6.2"), Ordering::Equal);
    }

    #[test]
    fn strips_v_prefix() {
        assert_eq!(cmp("v4.8.0", "4.8.0"), Ordering::Equal);
        assert!(is_newer("v4.8.0", "v4.7.2"));
    }

    #[test]
    fn date_based_ordering() {
        // 2025.11.20 is newer than 2025.9.4 (numeric, not lexical)
        assert!(is_newer("2025.11.20", "2025.9.4"));
    }

    #[test]
    fn bepinex_be_builds() {
        assert!(is_newer("6.0.0-be.735", "6.0.0-be.697"));
        assert_eq!(cmp("6.0.0-be.735", "6.0.0-be.735"), Ordering::Equal);
    }

    #[test]
    fn stable_outranks_prerelease() {
        assert!(is_newer("6.0.0", "6.0.0-be.764"));
        assert!(!is_newer("6.0.0-be.764", "6.0.0"));
    }

    #[test]
    fn bare_be_markers_order_numerically() {
        assert!(is_newer("be.770", "be.764"));
        assert_eq!(cmp("be.764", "be.764"), Ordering::Equal);
    }
}
