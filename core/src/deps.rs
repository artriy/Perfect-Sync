use crate::catalog::Catalog;
use std::collections::HashSet;

#[derive(Debug, PartialEq, Eq)]
pub struct Resolved {
    /// install order: each dependency appears before the mod that needs it
    pub ordered: Vec<String>,
}

fn visit(cat: &Catalog, id: &str, out: &mut Vec<String>, stack: &mut HashSet<String>) {
    if out.iter().any(|x| x == id) || !stack.insert(id.to_string()) {
        return;
    }
    if let Some(entry) = cat.get(id) {
        for dep in &entry.dependencies {
            visit(cat, dep, out, stack);
        }
    }
    stack.remove(id);
    out.push(id.to_string());
}

pub fn resolve(cat: &Catalog, selected: &[String]) -> Resolved {
    let mut ordered = Vec::new();
    let mut stack = HashSet::new();
    for id in selected {
        visit(cat, id, &mut ordered, &mut stack);
    }
    Resolved { ordered }
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
    }

    #[test]
    fn tohe_pulls_no_reactor() {
        let cat = parse(SAMPLE).unwrap();
        let r = resolve(&cat, &["EnhancedNetwork/TownofHost-Enhanced".to_string()]);
        assert!(!r.ordered.iter().any(|x| x == "NuclearPowered/Reactor"));
    }

    #[test]
    fn keeps_multiple_role_mods_in_install_order() {
        let cat = parse(SAMPLE).unwrap();
        let selected = [
            "AU-Avengers/TOU-Mira".to_string(),
            "EnhancedNetwork/TownofHost-Enhanced".to_string(),
        ];
        let resolved = resolve(&cat, &selected);
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
    fn cyclic_dependencies_terminate() {
        let json = r#"{"schema":1,"mods":[
            {"id":"A","name":"A","summary":"","repo":null,"tags":[],"dependencies":["B"],"assetRules":{}},
            {"id":"B","name":"B","summary":"","repo":null,"tags":[],"dependencies":["A"],"assetRules":{}}
        ]}"#;
        let cat = parse(json).unwrap();
        let r = resolve(&cat, &["A".to_string()]);
        assert!(r.ordered.iter().any(|x| x == "A"));
        assert!(r.ordered.iter().any(|x| x == "B"));
        assert_eq!(r.ordered.iter().filter(|x| *x == "A").count(), 1);
    }
}
