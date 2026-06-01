use crate::catalog::Catalog;
use crate::types::ModTag;

#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    /// install order: each dependency appears before the mod that needs it
    pub ordered: Vec<String>,
    /// ids of role mods when more than one is selected (cannot coexist)
    pub conflicts: Vec<String>,
}

fn visit(cat: &Catalog, id: &str, out: &mut Vec<String>) {
    if out.iter().any(|x| x == id) {
        return;
    }
    if let Some(entry) = cat.get(id) {
        for dep in &entry.dependencies {
            visit(cat, dep, out);
        }
    }
    out.push(id.to_string());
}

pub fn resolve(cat: &Catalog, selected: &[String]) -> Resolved {
    let mut ordered = Vec::new();
    for id in selected {
        visit(cat, id, &mut ordered);
    }
    let role_mods: Vec<String> = selected
        .iter()
        .filter(|id| {
            cat.get(id)
                .map(|e| e.tags.contains(&ModTag::Role))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    let conflicts = if role_mods.len() > 1 { role_mods } else { Vec::new() };
    Resolved { ordered, conflicts }
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
    fn expands_deps_before_dependent() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(&cat, &["AU-Avengers/TOU-Mira".to_string()]);
        // Reactor before MiraAPI before TOU-Mira
        assert!(idx(&r.ordered, "NuclearPowered/Reactor") < idx(&r.ordered, "All-Of-Us-Mods/MiraAPI"));
        assert!(idx(&r.ordered, "All-Of-Us-Mods/MiraAPI") < idx(&r.ordered, "AU-Avengers/TOU-Mira"));
        assert!(r.conflicts.is_empty());
    }

    #[test]
    fn tohe_pulls_no_reactor() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(&cat, &["EnhancedNetwork/TownofHost-Enhanced".to_string()]);
        assert!(!r.ordered.iter().any(|x| x == "NuclearPowered/Reactor"));
    }

    #[test]
    fn flags_two_role_mods() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(
            &cat,
            &[
                "AU-Avengers/TOU-Mira".to_string(),
                "EnhancedNetwork/TownofHost-Enhanced".to_string(),
            ],
        );
        assert_eq!(r.conflicts.len(), 2);
    }
}
