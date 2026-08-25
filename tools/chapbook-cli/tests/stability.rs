//! Keep `docs/STABILITY.md` honest about what is in the workspace.
//!
//! A stability policy that silently omits a crate is worse than none: the
//! omission reads as "no promises here" when what happened is that
//! somebody added a member and did not think about the tier. This is the
//! cheapest possible guard — every workspace member is named in the
//! policy, and every crate the policy names still exists.
//!
//! It lives in the workspace's tool crate because the thing under test is
//! the workspace, not any one library in it.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    let path: PathBuf = root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Workspace members, as the paths they are declared under — the `members`
/// array of the root manifest, read as text rather than parsed, because a
/// TOML dependency for one assertion is a worse trade than a `split`.
fn members() -> Vec<String> {
    let manifest = read("Cargo.toml");
    let (_, rest) = manifest.split_once("members = [").expect("members array");
    let (list, _) = rest.split_once(']').expect("members array end");
    list.lines()
        .filter_map(|line| {
            let line = line.trim().trim_end_matches(',');
            let name = line.strip_prefix('"')?.strip_suffix('"')?;
            Some(name.to_string())
        })
        .collect()
}

/// The crate name a member path declares, so a renamed directory cannot
/// pass by matching a stale entry in the doc.
fn crate_name(member: &str) -> String {
    let manifest = read(&format!("{member}/Cargo.toml"));
    manifest
        .lines()
        .find_map(|line| line.strip_prefix("name = "))
        .expect("name")
        .trim()
        .trim_matches('"')
        .to_string()
}

#[test]
fn every_workspace_member_has_a_stability_tier() {
    let policy = read("docs/STABILITY.md");
    let members = members();
    assert!(members.len() >= 15, "suspiciously few members: {members:?}");

    let missing: Vec<_> = members
        .iter()
        .filter(|member| {
            let name = crate_name(member);
            // Tools are named by path in the table, libraries by crate name.
            !policy.contains(&format!("`{name}`")) && !policy.contains(&format!("`{member}`"))
        })
        .cloned()
        .collect();
    assert!(
        missing.is_empty(),
        "not in docs/STABILITY.md: {missing:?} — pick a tier for it"
    );
}

#[test]
fn the_stability_policy_names_no_crate_that_left() {
    let policy = read("docs/STABILITY.md");
    let members = members();
    let known: Vec<String> = members.iter().map(|m| crate_name(m)).collect();

    // Every `chapbook-…` in backticks in the doc must still be a member.
    // The tier table and the prose both use that form, so this covers the
    // whole document rather than one table. Pairing backticks this way
    // needs the doc to have no fenced blocks, which is worth asserting
    // rather than silently mis-scanning if one ever lands.
    assert!(
        !policy.contains("```"),
        "docs/STABILITY.md grew a fenced code block; this scan pairs single \
         backticks and would mis-read it"
    );
    let mut stale = Vec::new();
    for chunk in policy.split('`').skip(1).step_by(2) {
        let name = chunk.trim_start_matches("tools/");
        if name.starts_with("chapbook-") && !known.iter().any(|k| k == name) {
            stale.push(name.to_string());
        }
    }
    stale.sort();
    stale.dedup();
    assert!(
        stale.is_empty(),
        "docs/STABILITY.md names crates that are not workspace members: {stale:?}"
    );
}

/// The three Internal-tier crates say so in their own rustdoc, because
/// that is where somebody about to depend on one is actually looking.
#[test]
fn internal_crates_say_so_at_the_top_of_their_docs() {
    for crate_dir in ["chapbook-dom", "chapbook-style", "chapbook-layout"] {
        let lib = read(&format!("crates/{crate_dir}/src/lib.rs"));
        let first = lib.lines().next().unwrap_or_default();
        assert!(
            first.contains("Internal to chapbook"),
            "{crate_dir}/src/lib.rs should open with the internal-tier banner, got: {first:?}"
        );
    }
}

/// Sanity: the doc lives where README points at it.
#[test]
fn the_readme_points_at_the_policy() {
    assert!(Path::new(&root().join("docs/STABILITY.md")).exists());
    assert!(read("README.md").contains("docs/STABILITY.md"));
}
