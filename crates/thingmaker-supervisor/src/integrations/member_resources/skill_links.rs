//! Skill-to-skill references. A skill refers to another only when its text
//! says so in a way that can be nothing else:
//!
//! * `plugin:skill`, where that plugin is installed and has that skill;
//! * a backticked name beside the word skill: "the `review` skill",
//!   "skill `review`".
//!
//! A bare word, a backticked word alone, a `/path` or "this skill documents"
//! never count: each of them matched ordinary prose and pulled in skills that
//! had nothing to do with the one selected.
//!
//! A reference is followed only inside the referring skill's own provider, to
//! the one definition it can mean: one in the same group, else one from the
//! same provider. Two candidates are a guess, and a guess is left out.
//!
//! References are shown in the selector. They change what a member loads only
//! when its selection asks for it (`referenced_skills`), and never for a skill
//! the person took out again (`excluded_skills`).

use std::{collections::{BTreeMap, BTreeSet}, sync::LazyLock};

use regex::Regex;

use super::{ResourceCatalog, ResourceEntry, ResourceSelection};

const NAME: &str = r"[A-Za-z0-9][A-Za-z0-9._-]*";
/// Both halves of `plugin:skill` start with a letter: `10:30` and
/// `localhost:3000` are not even looked up.
const WORD: &str = r"[A-Za-z][A-Za-z0-9._-]*";
static PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| [
    format!(r"(?:^|[^A-Za-z0-9._/:-])/?({WORD}:{WORD})"),
    format!(r"(?i)`/?({NAME})`\s+skills?\b"),
    format!(r"(?i)\bskills?\s+(?:called\s+|named\s+)?`/?({NAME})`"),
].iter().map(|pattern| Regex::new(pattern).expect("valid skill reference pattern")).collect());

fn short_name(entry: &ResourceEntry) -> String {
    entry.name.rsplit(':').next().unwrap_or_default().to_lowercase()
}

/// Every explicit reference in a skill's text, as (plugin, name).
fn references(text: &str) -> BTreeSet<(Option<String>, String)> {
    let mut found = BTreeSet::new();
    for pattern in PATTERNS.iter() {
        for capture in pattern.captures_iter(text) {
            // Sentence punctuation is not part of a name: "then studio:spec."
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
                .filter(|&other| other != position && entries[other].skill_path != entry.skill_path && entries[other].provider == entry.provider)
                .filter(|&other| plugin.as_ref().is_none_or(|plugin| entries[other].plugin.as_deref().map(str::to_lowercase).as_ref() == Some(plugin)))
                .collect();
            let near: Vec<usize> = candidates.iter().copied().filter(|&other| entries[other].group == entry.group).collect();
            let chosen = if near.is_empty() { &candidates } else { &near };
            if let [only] = chosen[..] { found.insert(entries[only].id.clone()); }
        }
        links.push((position, found));
    }
    for (position, found) in links { entries[position].skill_dependencies = found.into_iter().collect(); }
}

/// The skills a selection loads: the ones the person picked and, when the
/// selection asks for it, every available skill those refer to, except the
/// ones taken out again and what only they lead to. Picked ids come first, in
/// their order; the others follow, sorted.
pub fn expand_skills(catalog: &ResourceCatalog, selection: &ResourceSelection) -> Vec<String> {
    let explicit = &selection.skills;
    if !selection.referenced_skills { return explicit.clone() }
    let by_id: BTreeMap<&str, &ResourceEntry> = catalog.entries.iter().filter(|entry| entry.kind == "skill").map(|entry| (entry.id.as_str(), entry)).collect();
    let mut seen: BTreeSet<String> = explicit.iter().chain(&selection.excluded_skills).cloned().collect();
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
    fn only_unmistakable_mentions_are_references() {
        let found = references("Use the `review` skill, or the skill named `plan`. Call studio:spec. Skills `a` and `b`.");
        let expected = [(None, "review"), (None, "plan"), (Some("studio"), "spec"), (None, "a")];
        assert_eq!(found, expected.iter().map(|(plugin, name)| (plugin.map(str::to_string), name.to_string())).collect());
        // Each of these was taken for a reference once, and none is one.
        for prose in [
            "The Graph API this skill documents.", "This skill files a report.", "Each skill is a folder.", "A skill for reviews.",
            "The review skill is good.", "Then run /ship.", "Open `ui` and `docs`.", "Read /docs/plan.md and `/api/health`.",
            "See https://example.test/review and file:///tmp/plan.", "At 10:30 run the tests.", "Run `review` first.",
        ] {
            assert!(references(prose).is_empty(), "{prose} → {:?}", references(prose));
        }
    }

    #[test]
    fn a_reference_is_followed_only_to_the_one_skill_it_can_mean() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let skills = config.join("skills");
        let skill = |name: &str, body: &str| format!("---\nname: {name}\ndescription: {name}\n---\n{body}");
        write(&skills.join("ship/SKILL.md"), &skill("ship", "Uses the `review` skill, the `documents` skill, the `twin` skill and art:draw. This skill documents releases. It imports node:fs and sets type:module."));
        write(&skills.join("review/SKILL.md"), &skill("review", "See the `lint` skill."));
        write(&skills.join("lint/SKILL.md"), &skill("lint", "Back to the `ship` skill."));
        // Another provider's skill of a name this one mentions: not followed.
        write(&codex.join("skills/documents/SKILL.md"), &skill("documents", ""));
        write(&home.join(".gemini/skills/review/SKILL.md"), &skill("review", ""));
        // Two skills of the same provider and group answer to `twin`: a guess, left out.
        write(&skills.join("twin-a/SKILL.md"), &skill("twin", ""));
        write(&skills.join("twin-b/SKILL.md"), &skill("twin", ""));
        // `art:draw` is followed because that plugin is installed and has that skill.
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art"}"#);
        write(&plugin.join("skills/draw/SKILL.md"), &skill("draw", ""));
        write(&config.join("plugins/installed_plugins.json"), &serde_json::json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":plugin}]}}).to_string());
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);

        let catalog = catalog_with_homes(home, &config, &codex, None).unwrap();
        let ship = catalog.entries.iter().find(|entry| entry.id == "skill:user:ship").unwrap();
        assert_eq!(ship.skill_dependencies, ["skill:art@market:draw", "skill:user:review"]);
    }

    #[test]
    fn references_load_nothing_until_the_selection_asks_and_never_what_was_taken_out() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let skills = config.join("skills");
        let skill = |name: &str, body: &str| format!("---\nname: {name}\ndescription: {name}\n---\n{body}");
        write(&skills.join("ship/SKILL.md"), &skill("ship", "Uses the `review` skill and the `notes` skill."));
        write(&skills.join("review/SKILL.md"), &skill("review", "See the `lint` skill."));
        write(&skills.join("lint/SKILL.md"), &skill("lint", "Back to the `ship` skill."));
        write(&skills.join("notes/SKILL.md"), &skill("notes", ""));
        let catalog = catalog_with_homes(home, &config, &codex, None).unwrap();

        // A team saved before references existed: exactly what it selected.
        let saved: ResourceSelection = serde_json::from_str(r#"{"skills":["skill:user:ship"],"mcps":[]}"#).unwrap();
        assert_eq!(expand_skills(&catalog, &saved), ["skill:user:ship"]);
        assert_eq!(resolve(&catalog, &saved, &Default::default()).unwrap().skills, ["ship"]);
        assert_eq!(serde_json::to_string(&saved).unwrap(), r#"{"skills":["skill:user:ship"],"mcps":[]}"#, "and it is stored as it was");

        let asked = ResourceSelection { referenced_skills: true, ..saved.clone() };
        assert_eq!(expand_skills(&catalog, &asked), ["skill:user:ship", "skill:user:lint", "skill:user:notes", "skill:user:review"], "cycles end; picked first");
        assert_eq!(resolve(&catalog, &asked, &Default::default()).unwrap().skills, ["lint", "notes", "review", "ship"]);
        // Taken out: neither it nor what only it led to.
        let trimmed = ResourceSelection { excluded_skills: vec!["skill:user:review".into()], ..asked.clone() };
        assert_eq!(expand_skills(&catalog, &trimmed), ["skill:user:ship", "skill:user:notes"]);
        let stored = serde_json::to_string(&trimmed).unwrap();
        assert_eq!(stored, r#"{"skills":["skill:user:ship"],"mcps":[],"referencedSkills":true,"excludedSkills":["skill:user:review"]}"#);
        assert_eq!(serde_json::from_str::<ResourceSelection>(&stored).unwrap(), trimmed);
    }

    #[test]
    fn a_referenced_skill_needs_its_connection_like_a_picked_one() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        write(&config.join("skills/ship/SKILL.md"), "---\nname: ship\ndescription: Ships\n---\nUses the `board` skill.");
        write(&config.join("skills/board/SKILL.md"), "---\nname: board\ndescription: Board\nmcp-servers: [tracker]\n---\n");
        write(&home.join(".claude.json"), r#"{"mcpServers":{"tracker":{"command":"tracker"}}}"#);
        let catalog = catalog_with_homes(home, &config, &codex, None).unwrap();
        let picked = ResourceSelection { skills: vec!["skill:user:ship".into()], ..Default::default() };
        assert!(resolve(&catalog, &picked, &Default::default()).is_ok(), "not asked for, not loaded, nothing needed");
        let asked = ResourceSelection { referenced_skills: true, ..picked.clone() };
        let refused = resolve(&catalog, &asked, &Default::default()).err().unwrap();
        assert!(refused.message.contains("Skill board requires MCP tracker"), "{}", refused.message);
        let connected = ResourceSelection { mcps: vec!["mcp:user:tracker".into()], ..asked.clone() };
        assert_eq!(resolve(&catalog, &connected, &Default::default()).unwrap().skills, ["board", "ship"]);
        let without = ResourceSelection { excluded_skills: vec!["skill:user:board".into()], ..asked };
        assert_eq!(resolve(&catalog, &without, &Default::default()).unwrap().skills, ["ship"]);
    }
}
