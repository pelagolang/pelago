//! The grouped `std` tree stays complete: every module is in exactly one group, and no group names a
//! module that does not exist.

use pelago::package::{std_tree_json, STD_GROUPS, STD_MODULES};

#[test]
fn every_std_module_is_in_exactly_one_group() {
    for (path, _) in STD_MODULES {
        let n = STD_GROUPS
            .iter()
            .filter(|(_, _, mods)| mods.contains(path))
            .count();
        assert_eq!(
            n, 1,
            "std.{path} must be in exactly one group of STD_GROUPS"
        );
    }
    for (group, _, mods) in STD_GROUPS {
        for m in *mods {
            assert!(
                STD_MODULES.iter().any(|(p, _)| p == m),
                "group {group} lists std.{m}, which is not in STD_MODULES"
            );
        }
    }
}

#[test]
fn the_tree_prints_as_json() {
    let j = std_tree_json();
    assert!(j.starts_with("[{\"group\":\"system\""));
    assert!(j.contains("\"std.json\""));
}
