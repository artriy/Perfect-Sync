use crate::catalog::Catalog;
use std::collections::{HashMap, HashSet};

#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    /// Install order: each dependency appears before the mod that needs it.
    pub ordered: Vec<String>,
    /// Semver constraints declared by every selected dependent, keyed by the
    /// dependency's canonical catalog id.
    pub requirements: HashMap<String, Vec<String>>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DependencyError {
    #[error("catalog dependency {dependency:?} required by {required_by:?} is missing")]
    Missing {
        required_by: String,
        dependency: String,
    },
    #[error("dependency cycle includes {0:?}")]
    Cycle(String),
}

/// Resolve dependencies without recursion. Identifiers are matched using the
/// same ASCII case-insensitive semantics as catalog lookup, and output uses the
/// catalog's canonical spelling.
pub fn resolve(cat: &Catalog, selected: &[String]) -> Result<Resolved, DependencyError> {
    let mut ordered = Vec::new();
    let mut done = HashSet::new();
    let mut active = HashSet::new();
    let mut requirements: HashMap<String, Vec<String>> = HashMap::new();

    for requested in selected {
        let Some(root) = cat.get(requested) else {
            return Err(DependencyError::Missing {
                required_by: "selection".to_string(),
                dependency: requested.clone(),
            });
        };
        let root_id = root.id.clone();
        if done.contains(&root_id.to_ascii_lowercase()) {
            continue;
        }

        // `expanded` frames append the node after all of its dependencies.
        let mut stack = vec![(root_id, false)];
        while let Some((id, expanded)) = stack.pop() {
            let folded = id.to_ascii_lowercase();
            if expanded {
                active.remove(&folded);
                if done.insert(folded) {
                    ordered.push(id);
                }
                continue;
            }
            if done.contains(&folded) {
                continue;
            }
            if !active.insert(folded.clone()) {
                return Err(DependencyError::Cycle(id));
            }

            let entry = cat.get(&id).ok_or_else(|| DependencyError::Missing {
                required_by: "selection".to_string(),
                dependency: id.clone(),
            })?;
            stack.push((entry.id.clone(), true));
            for dependency in entry.dependencies.iter().rev() {
                let Some(target) = cat.get(dependency) else {
                    return Err(DependencyError::Missing {
                        required_by: entry.id.clone(),
                        dependency: dependency.clone(),
                    });
                };
                if let Some(requirement) = entry.dependency_versions.get(&target.id) {
                    let constraints = requirements.entry(target.id.clone()).or_default();
                    if !constraints.contains(requirement) {
                        constraints.push(requirement.clone());
                    }
                }
                let target_folded = target.id.to_ascii_lowercase();
                if active.contains(&target_folded) {
                    return Err(DependencyError::Cycle(target.id.clone()));
                }
                if !done.contains(&target_folded) {
                    stack.push((target.id.clone(), false));
                }
            }
        }
    }

    Ok(Resolved {
        ordered,
        requirements,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::parse;

    const SAMPLE: &str = include_str!("../fixtures/catalog.sample.json");

    fn idx(v: &[String], id: &str) -> usize {
        v.iter().position(|x| x == id).expect("present")
    }

    #[test]
    fn expands_deps_before_dependent_case_insensitively() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(&cat, &["au-avengers/tou-mira".to_string()]).unwrap();
        assert!(
            idx(&r.ordered, "NuclearPowered/Reactor") < idx(&r.ordered, "All-Of-Us-Mods/MiraAPI")
        );
        assert!(
            idx(&r.ordered, "All-Of-Us-Mods/MiraAPI") < idx(&r.ordered, "AU-Avengers/TOU-Mira")
        );
    }

    #[test]
    fn keeps_multiple_role_mods_in_install_order() {
        let cat = parse(SAMPLE).unwrap();
        let selected = [
            "AU-Avengers/TOU-Mira".to_string(),
            "EnhancedNetwork/TownofHost-Enhanced".to_string(),
        ];
        let resolved = resolve(&cat, &selected).unwrap();
        assert!(resolved
            .ordered
            .iter()
            .any(|id| id == "AU-Avengers/TOU-Mira"));
        assert!(resolved
            .ordered
            .iter()
            .any(|id| id == "EnhancedNetwork/TownofHost-Enhanced"));
    }

    #[test]
    fn aggregates_version_requirements_for_shared_dependencies() {
        let catalog = parse(r#"{"schema":1,"mods":[
            {"id":"A/One","name":"One","summary":"s","repo":null,"tags":[],"dependencies":["D/Shared"],"dependencyVersions":{"D/Shared":">=2.0.0"},"assetRules":{}},
            {"id":"B/Two","name":"Two","summary":"s","repo":null,"tags":[],"dependencies":["D/Shared"],"dependencyVersions":{"D/Shared":"<3.0.0"},"assetRules":{}},
            {"id":"D/Shared","name":"Shared","summary":"s","repo":null,"tags":[],"dependencies":[],"assetRules":{}}
        ]}"#).unwrap();
        let resolved = resolve(&catalog, &["A/One".into(), "B/Two".into()]).unwrap();
        assert_eq!(
            resolved.requirements.get("D/Shared").unwrap(),
            &[">=2.0.0", "<3.0.0"]
        );
    }

    #[test]
    fn reports_missing_and_cycle_explicitly() {
        let mut missing = parse(SAMPLE).unwrap();
        missing.mods[0].dependencies.push("Missing/Library".into());
        assert!(matches!(
            resolve(&missing, &[missing.mods[0].id.clone()]),
            Err(DependencyError::Missing { .. })
        ));

        let mut cyclic = parse(SAMPLE).unwrap();
        let cycle_target = cyclic.mods[0].id.clone();
        cyclic.mods[2].dependencies.push(cycle_target);
        assert!(matches!(
            resolve(&cyclic, &[cyclic.mods[0].id.clone()]),
            Err(DependencyError::Cycle(_))
        ));
    }
}
