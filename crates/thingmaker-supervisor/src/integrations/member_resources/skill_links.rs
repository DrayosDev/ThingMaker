//! Skill-to-skill references. A skill depends on another when its description
//! or body names it explicitly: `plugin:skill`, a backticked name, a `/name`
//! command, or "the X skill" / "skill X". Bare words never count, so a skill
//! called `run` is not pulled in by every sentence that says "run".
//!
//! A name resolves to the closest definition: the same provider and group, then
//! the same provider, then anywhere, but only when that last step is unambiguous.

use std::{collections::{BTreeMap, BTreeSet}, sync::LazyLock};

use regex::Regex;

use super::{ResourceCatalog, ResourceEntry};

const NAME: &str = r"[A-Za-z0-9][A-Za-z0-9._-]*";
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| [
    format!(r"(?:^|[^A-Za-z0-9._/-])/?({NAME}:{NAME})"),
    format!(r"`/?({NAME}(?::{NAME})?)`"),
    format!(r"(?:^|[\s(\[])/({NAME}(?::{NAME})?)"),
    format!(r"(?i)\b({NAME}(?::{NAME})?)[`'\x22]?\s+skill\b"),
    format!(r"(?i)\bskill\s+(?:called\s+|named\s+)?[`'\x22]?({NAME}(?::{NAME})?)"),
].iter().map(|pattern| Regex::new(pattern).expect("valid skill reference pattern")).collect());

fn short_name(entry: &ResourceEntry) -> String {
    entry.name.rsplit(':').next().unwrap_or_default().to_lowercase()
}

/// Every explicit reference in a skill's text, as (plugin, name).
fn references(text: &str) -> BTreeSet<(Option<String>, String)> {
    let mut found = BTreeSet::new();
    for pattern in PATTERNS.iter() {
        for capture in pattern.captures_iter(text) {
            // Sentence punctuation is not part of a name: "then /ship."
            let token = capture[1].trim_end_matches(['.', '-', '_']).to_lowercase();
            let reference = match token.split_once(':') {
                Some((plugin, name)) => (Some(plugin.to_string()), name.to_string()),
                None => (None, token),
            };
            found.insert(reference);
        }
    }
    found
}

/// Fills each skill's `skill_dependencies` with the ids it refers to.
pub(super) fn annotate(entries: &mut [ResourceEntry]) {
    let mut by_name: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (position, entry) in entries.iter().enumerate().filter(|(_, entry)| entry.kind == "skill") {
        by_name.entry(short_name(entry)).or_default().push(position);
    }
    let mut links = Vec::new();
    for (position, entry) in entries.iter().enumerate().filter(|(_, entry)| entry.kind == "skill") {
        let Some(text) = entry.skill_path.as_ref().and_then(|path| std::fs::read_to_string(path).ok()) else { continue };
        let mut found = BTreeSet::new();
        for (plugin, name) in references(&text) {
            let Some(candidates) = by_name.get(&name) else { continue };
            let candidates: Vec<usize> = candidates.iter().copied()
                .filter(|&other| other != position && entries[other].skill_path != entry.skill_path)
                .filter(|&other| plugin.as_ref().is_none_or(|plugin| entries[other].plugin.as_deref().map(str::to_lowercase).as_ref() == Some(plugin)))
                .collect();
            let tiers = [
                candidates.iter().copied().filter(|&other| entries[other].provider == entry.provider && entries[other].group == entry.group).collect::<Vec<_>>(),
                candidates.iter().copied().filter(|&other| entries[other].provider == entry.provider).collect(),
                candidates.clone(),
            ];
            let Some((tier, chosen)) = tiers.iter().enumerate().find(|(_, tier)| !tier.is_empty()) else { continue };
            let available: Vec<usize> = chosen.iter().copied().filter(|&other| entries[other].available).collect();
            let pick = if available.is_empty() { chosen } else { &available };
            // Close definitions are alternatives of one another; distant ones
            // from several providers are a guess, so they are left out.
            if pick.len() == 1 || tier < 2 { found.insert(entries[pick[0]].id.clone()); }
        }
        links.push((position, found));
    }
    for (position, found) in links { entries[position].skill_dependencies = found.into_iter().collect(); }
}

/// The explicitly selected skills plus every available skill they reach.
/// Explicit ids come first, in their order; implied ones follow, sorted.
pub fn expand_skills(catalog: &ResourceCatalog, explicit: &[String]) -> Vec<String> {
    let by_id: BTreeMap<&str, &ResourceEntry> = catalog.entries.iter().filter(|entry| entry.kind == "skill").map(|entry| (entry.id.as_str(), entry)).collect();
    let mut seen: BTreeSet<String> = explicit.iter().cloned().collect();
    let mut pending: Vec<&str> = explicit.iter().map(String::as_str).collect();
    let mut implied = BTreeSet::new();
    while let Some(id) = pending.pop() {
        let Some(entry) = by_id.get(id) else { continue };
        for dependency in &entry.skill_dependencies {
            if by_id.get(dependency.as_str()).is_some_and(|other| other.available) && seen.insert(dependency.clone()) {
                implied.insert(dependency.clone());
                pending.push(dependency.as_str());
            }
        }
    }
    explicit.iter().cloned().chain(implied).collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::super::{ResourceSelection, catalog_with_homes, resolve};
    use super::{expand_skills, references};

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn only_explicit_mentions_are_references() {
        let found = references("Use the `review` skill, then /ship. Call unity-dev:spec and skill named plan. We run tests.");
        for expected in [(None, "review"), (None, "ship"), (Some("unity-dev"), "spec"), (None, "plan")] {
            assert!(found.contains(&(expected.0.map(str::to_string), expected.1.to_string())), "{expected:?} in {found:?}");
        }
        assert!(!found.contains(&(None, "run".to_string())) && !found.contains(&(None, "tests".to_string())));
    }

    #[test]
    fn dependencies_resolve_nearby_and_expand_transitively() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let skills = config.join("skills");
        write(&skills.join("ship/SKILL.md"), "---\nname: ship\ndescription: Ships. Uses the `review` skill.\n---\nThen run /notes.");
        write(&skills.join("review/SKILL.md"), "---\nname: review\ndescription: Reviews\n---\nSee `lint`.");
        write(&skills.join("lint/SKILL.md"), "---\nname: lint\ndescription: Lints\n---\nBack to `ship`.");
        write(&skills.join("notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\n");
        // Same name elsewhere: the nearer (same group) definition wins.
        write(&home.join(".gemini/skills/review/SKILL.md"), "---\nname: review\ndescription: Other\n---\n");
        let catalog = catalog_with_homes(home, &config, &codex, None).unwrap();
        let ship = catalog.entries.iter().find(|entry| entry.id == "skill:user:ship").unwrap();
        assert_eq!(ship.skill_dependencies, ["skill:user:notes", "skill:user:review"]);
        let expanded = expand_skills(&catalog, &["skill:user:ship".into()]);
        assert_eq!(expanded, ["skill:user:ship", "skill:user:lint", "skill:user:notes", "skill:user:review"], "cycles end; explicit first");
        let resolved = resolve(&catalog, &ResourceSelection { skills: vec!["skill:user:ship".into()], mcps: vec![] }, &Default::default()).unwrap();
        assert_eq!(resolved.skills, ["lint", "notes", "review", "ship"]);
    }
}
