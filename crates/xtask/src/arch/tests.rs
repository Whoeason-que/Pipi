use super::*;

fn rules(entries: &[(&str, &[&str], &[&str], &[&str])]) -> BTreeMap<String, CrateRule> {
    entries
        .iter()
        .map(|(name, may, dev, forbid)| {
            (
                name.to_string(),
                CrateRule {
                    may_depend_on: may.iter().map(|item| item.to_string()).collect(),
                    dev_may_depend_on: dev.iter().map(|item| item.to_string()).collect(),
                    forbid: forbid.iter().map(|item| item.to_string()).collect(),
                },
            )
        })
        .collect()
}

fn crate_deps(deps: &[&str], dev: &[&str]) -> CrateDeps {
    CrateDeps {
        deps: deps.iter().map(|item| item.to_string()).collect(),
        dev_deps: dev.iter().map(|item| item.to_string()).collect(),
        build_deps: BTreeSet::new(),
    }
}

#[test]
fn allowed_edges_pass() {
    let rules = rules(&[("a", &[], &[], &[]), ("b", &["a"], &[], &[])]);
    let crates = BTreeMap::from([
        ("a".to_string(), crate_deps(&[], &[])),
        ("b".to_string(), crate_deps(&["a"], &[])),
    ]);
    assert!(check(&rules, &crates).is_empty());
}

#[test]
fn undeclared_internal_dependency_fails() {
    let rules = rules(&[("a", &[], &[], &[]), ("b", &[], &[], &[])]);
    let crates = BTreeMap::from([
        ("a".to_string(), crate_deps(&[], &[])),
        ("b".to_string(), crate_deps(&["a"], &[])),
    ]);
    assert_eq!(
        check(&rules, &crates),
        vec![Violation::Undeclared {
            krate: "b".into(),
            dep: "a".into(),
            dev: false,
        }]
    );
}

#[test]
fn forbidden_third_party_fails() {
    let rules = rules(&[("a", &[], &[], &["reqwest"])]);
    let crates = BTreeMap::from([("a".to_string(), crate_deps(&["reqwest"], &[]))]);
    assert_eq!(
        check(&rules, &crates),
        vec![Violation::Forbidden {
            krate: "a".into(),
            dep: "reqwest".into(),
        }]
    );
}

#[test]
fn dev_dependency_needs_its_own_allowance() {
    let rules = rules(&[("a", &[], &[], &[]), ("b", &[], &[], &[])]);
    let crates = BTreeMap::from([
        ("a".to_string(), crate_deps(&[], &[])),
        ("b".to_string(), crate_deps(&[], &["a"])),
    ]);
    assert_eq!(
        check(&rules, &crates),
        vec![Violation::Undeclared {
            krate: "b".into(),
            dep: "a".into(),
            dev: true,
        }]
    );
}

#[test]
fn cycle_is_reported() {
    let rules = rules(&[("a", &["b"], &[], &[]), ("b", &["a"], &[], &[])]);
    let crates = BTreeMap::from([
        ("a".to_string(), crate_deps(&["b"], &[])),
        ("b".to_string(), crate_deps(&["a"], &[])),
    ]);
    let violations = check(&rules, &crates);
    assert!(
        violations
            .iter()
            .any(|violation| matches!(violation, Violation::Cycle(_))),
        "expected a cycle violation, got {violations:?}"
    );
}

#[test]
fn unregistered_member_and_unknown_entry_are_both_reported() {
    let rules = rules(&[("ghost", &[], &[], &[])]);
    let crates = BTreeMap::from([("a".to_string(), crate_deps(&[], &[]))]);
    let violations = check(&rules, &crates);
    assert!(violations.contains(&Violation::Unregistered("a".into())));
    assert!(violations.contains(&Violation::UnknownCrate("ghost".into())));
}
