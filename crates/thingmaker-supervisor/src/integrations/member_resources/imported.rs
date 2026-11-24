//! Imported skills: a copy of every skill a provider offers, local or from a
//! plugin, under `~/.thingmaker/imported-skills/<provider>/`. Once a provider is
//! set up, the catalog lists those copies in place of the provider's own skills
//! (same ids, so saved selections keep working) and re-syncs them each time it
//! is built: a copy is refreshed when its source's fingerprint changes and
//! removed when its source is gone.
//!
//! A plugin skill is copied with its whole plugin root, because skills reach
//! shared scripts through paths such as `../../scripts`. Project skills stay
//! native: they belong to one workspace, not to the provider.

use std::{collections::BTreeMap, path::{Path, PathBuf}};

use serde::{Deserialize, Serialize};

use crate::{DesktopError, storage::content_hash};

use super::{ResourceEntry, bridge_skills};

/// The providers whose skills can be imported.
pub const PROVIDERS: [&str; 3] = ["claude", "codex", "gemini"];
const INDEX: &str = "index.json";
/// Never copied: version control, dependencies and the provider's own markers.
const SKIPPED: [&str; 5] = [".git", "node_modules", ".in_use", "__pycache__", ".DS_Store"];
/// A larger unit stays native rather than filling the disk with copies.
const MAX_UNIT_BYTES: u64 = 200 * 1024 * 1024;

/// One provider's import, as the selector shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportStatus {
    pub provider: String,
    pub imported: bool,
    pub skills: usize,
    /// Unix milliseconds of the last completed sync.
    pub synced_at: Option<i64>,
    pub problem: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    synced_at: Option<i64>,
    /// Copy folder name → what it was copied from.
    units: BTreeMap<String, Unit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Unit {
    source: PathBuf,
    fingerprint: String,
}

pub fn root(home: &Path) -> PathBuf {
    home.join(".thingmaker").join("imported-skills")
}

fn now_millis() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|elapsed| elapsed.as_millis() as i64).unwrap_or_default()
}

fn valid_provider(provider: &str) -> Result<(), DesktopError> {
    if PROVIDERS.contains(&provider) { Ok(()) } else { Err(DesktopError::unsupported("Skills cannot be imported from this provider")) }
}

/// Starts importing a provider: the next catalog build copies its skills.
pub fn setup(root: &Path, provider: &str) -> Result<(), DesktopError> {
    valid_provider(provider)?;
    let directory = root.join(provider);
    std::fs::create_dir_all(&directory).map_err(|error| DesktopError::io(format!("Cannot create {}: {error}", directory.display())))?;
    if !directory.join(INDEX).is_file() { write_index(&directory, &Index::default())?; }
    Ok(())
}

fn read_index(directory: &Path) -> Option<Index> {
    std::fs::read(directory.join(INDEX)).ok().map(|bytes| serde_json::from_slice(&bytes).unwrap_or_default())
}

fn write_index(directory: &Path, index: &Index) -> Result<(), DesktopError> {
    let temp = directory.join(format!(".{INDEX}.tmp-{}", std::process::id()));
    let text = serde_json::to_vec_pretty(index).map_err(|error| DesktopError::io(error.to_string()))?;
    std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, directory.join(INDEX)))
        .map_err(|error| DesktopError::io(format!("Cannot save the imported skills index: {error}")))
}

/// A stable, readable folder name for one copied unit.
fn unit_key(entry: &ResourceEntry, unit_root: &Path) -> String {
    let label = match &entry.plugin {
        Some(_) => entry.group.trim_start_matches("plugin:").to_string(),
        None => format!("{}-{}", entry.group, unit_root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()),
    };
    let label: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || "@._-".contains(c) { c } else { '-' }).collect();
    format!("{}-{}", label.trim_start_matches('.'), &content_hash(unit_root.to_string_lossy().as_bytes())[..8])
}

/// Walks a unit as an import sees it: skipped names left out, linked
/// directories not followed (they can loop), linked files followed.
fn walk(directory: &Path, relative: &Path, visit: &mut dyn FnMut(&Path, &Path, &std::fs::Metadata) -> std::io::Result<()>) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if SKIPPED.iter().any(|skipped| name == *skipped) { continue }
        let path = entry.path();
        let relative = relative.join(&name);
        if entry.file_type()?.is_dir() {
            walk(&path, &relative, visit)?;
        } else if let Ok(metadata) = std::fs::metadata(&path) && metadata.is_file() {
            visit(&path, &relative, &metadata)?;
        }
    }
    Ok(())
}

/// Path, size and modification time of every file: cheap enough to run on
/// each catalog build, and it changes whenever a plugin updates in place.
fn fingerprint(unit_root: &Path) -> std::io::Result<(String, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut total = 0;
    walk(unit_root, Path::new(""), &mut |_, relative, metadata| {
        let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|elapsed| elapsed.as_nanos()).unwrap_or_default();
        hasher.update(relative.to_string_lossy().as_bytes()).update(&[0]).update(&metadata.len().to_le_bytes()).update(&modified.to_le_bytes());
        total += metadata.len();
        Ok(())
    })?;
    Ok((hasher.finalize().to_hex().to_string(), total))
}

/// Copies beside the destination, then swaps, so a reader never sees half a copy.
fn copy_unit(source: &Path, directory: &Path, key: &str) -> std::io::Result<()> {
    let stamp = format!("{}-{}", std::process::id(), now_millis());
    let temp = directory.join(format!(".{key}.tmp-{stamp}"));
    let result = walk(source, Path::new(""), &mut |path, relative, _| {
        let target = temp.join(relative);
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::copy(path, target).map(|_| ())
    }).and_then(|_| std::fs::create_dir_all(&temp));
    if let Err(error) = result {
        let _ = std::fs::remove_dir_all(&temp);
        return Err(error);
    }
    let destination = directory.join(key);
    let old = directory.join(format!(".{key}.old-{stamp}"));
    if destination.exists() { std::fs::rename(&destination, &old)?; }
    std::fs::rename(&temp, &destination)?;
    let _ = std::fs::remove_dir_all(&old);
    Ok(())
}

/// Replaces each set-up provider's skills with their synced copies.
pub(super) fn apply(entries: &mut [ResourceEntry], root: &Path) -> Vec<ImportStatus> {
    PROVIDERS.iter().map(|provider| apply_provider(entries, root, provider)).collect()
}

fn apply_provider(entries: &mut [ResourceEntry], root: &Path, provider: &str) -> ImportStatus {
    let directory = root.join(provider);
    let mut status = ImportStatus { provider: provider.into(), imported: false, skills: 0, synced_at: None, problem: None };
    let Some(mut index) = read_index(&directory) else { return status };
    status.imported = true;
    let candidates: Vec<usize> = entries.iter().enumerate()
        .filter(|(_, entry)| entry.kind == "skill" && entry.provider.as_deref() == Some(provider) && entry.group != "project" && entry.skill_path.is_some() && entry.unit_root.is_some())
        .map(|(position, _)| position).collect();
    let mut units = BTreeMap::new();
    for &position in &candidates {
        let unit_root = entries[position].unit_root.clone().unwrap_or_default();
        units.entry(unit_key(&entries[position], &unit_root)).or_insert(unit_root);
    }
    let mut problems = Vec::new();
    let mut synced = BTreeMap::new();
    for (key, source) in &units {
        let copied = directory.join(key).is_dir();
        match fingerprint(source) {
            Ok((_, size)) if size > MAX_UNIT_BYTES => problems.push(format!("{} is larger than {} MB and stays native", source.display(), MAX_UNIT_BYTES / 1024 / 1024)),
            Ok((fingerprint, _)) => {
                let current = index.units.get(key).is_some_and(|unit| unit.fingerprint == fingerprint && unit.source == *source);
                if current && copied {
                    synced.insert(key.clone(), Unit { source: source.clone(), fingerprint });
                } else if let Err(error) = copy_unit(source, &directory, key) {
                    problems.push(format!("Cannot copy {}: {error}", source.display()));
                    // A stale copy is still a working one.
                    if let Some(unit) = index.units.get(key).filter(|_| copied) { synced.insert(key.clone(), unit.clone()); }
                } else {
                    synced.insert(key.clone(), Unit { source: source.clone(), fingerprint });
                }
            }
            Err(error) => {
                problems.push(format!("Cannot read {}: {error}", source.display()));
                if let Some(unit) = index.units.get(key).filter(|_| copied) { synced.insert(key.clone(), unit.clone()); }
            }
        }
    }
    // Copies whose source is gone. Keys come from the index this module wrote;
    // anything that is not a plain folder name is left alone.
    for key in index.units.keys().filter(|key| !synced.contains_key(*key)) {
        if !key.is_empty() && !key.starts_with('.') && !key.contains(['/', '\\']) {
            let _ = std::fs::remove_dir_all(directory.join(key));
        }
    }
    let changed = index.units.len() != synced.len() || synced.iter().any(|(key, unit)| index.units.get(key).is_none_or(|old| old.fingerprint != unit.fingerprint));
    for &position in &candidates {
        let entry = &mut entries[position];
        let (Some(unit_root), Some(skill_path)) = (entry.unit_root.clone(), entry.skill_path.clone()) else { continue };
        let key = unit_key(entry, &unit_root);
        let Some(relative) = synced.contains_key(&key).then(|| skill_path.strip_prefix(&unit_root).ok()).flatten() else { continue };
        let copy = directory.join(&key).join(relative);
        if !copy.is_file() { continue }
        entry.skill_path = Some(copy);
        entry.unit_root = Some(directory.join(&key));
        bridge_skills(std::slice::from_mut(entry), &format!("imported-skills/{provider}"));
        status.skills += 1;
    }
    if changed || index.synced_at.is_none() { index.synced_at = Some(now_millis()); }
    index.units = synced;
    if let Err(error) = write_index(&directory, &index) { problems.push(error.message); }
    status.synced_at = index.synced_at;
    status.problem = (!problems.is_empty()).then(|| problems.join("\n"));
    status
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::super::{ResourceSelection, catalog_with_homes, resolve};

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn set_up_providers_list_synced_copies_and_follow_their_sources() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\nv1");
        // A plugin skill that reaches a shared script through its plugin's root.
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art"}"#);
        write(&plugin.join("skills/draw/SKILL.md"), "---\nname: draw\ndescription: Draws\n---\nRun ../../scripts/draw.sh");
        write(&plugin.join("scripts/draw.sh"), "echo draw");
        write(&plugin.join("node_modules/huge/index.js"), "skipped");
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"user","installPath":plugin}]}}).to_string());
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        let root = super::root(home);

        let native = catalog_with_homes(home, &config, &codex, None).unwrap();
        let claude = native.imports.iter().find(|status| status.provider == "claude").unwrap();
        assert!(!claude.imported, "nothing is copied before setup");
        assert!(native.entries.iter().any(|entry| entry.source == ".claude/skills"));

        super::setup(&root, "claude").unwrap();
        let imported = catalog_with_homes(home, &config, &codex, None).unwrap();
        let status = imported.imports.iter().find(|status| status.provider == "claude").unwrap();
        assert!(status.imported && status.synced_at.is_some() && status.problem.is_none(), "{status:?}");
        assert_eq!(status.skills, 2);
        let draw = imported.entries.iter().find(|entry| entry.id == "skill:art@market:draw").unwrap();
        assert_eq!(draw.source, "imported-skills/claude");
        assert!(draw.bridge_skill && draw.available);
        let copy = draw.skill_path.as_ref().unwrap();
        assert!(copy.starts_with(root.join("claude")));
        let plugin_copy = copy.parent().unwrap().parent().unwrap().parent().unwrap();
        assert!(plugin_copy.join("scripts/draw.sh").is_file(), "the plugin root comes along");
        assert!(!plugin_copy.join("node_modules").exists());

        // The same ids resolve to the copies, so saved selections keep working.
        let selection = ResourceSelection { skills: vec!["skill:user:notes".into()], mcps: vec![] };
        let resolved = resolve(&imported, &selection, &Default::default()).unwrap();
        assert!(resolved.skill_sources[0].path.starts_with(root.join("claude").canonicalize().unwrap()));

        // A source edit is re-synced on the next build; a removed source drops its copy.
        std::thread::sleep(std::time::Duration::from_millis(20));
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\nv2");
        let resynced = catalog_with_homes(home, &config, &codex, None).unwrap();
        let notes = resynced.entries.iter().find(|entry| entry.id == "skill:user:notes").unwrap();
        assert!(std::fs::read_to_string(notes.skill_path.as_ref().unwrap()).unwrap().ends_with("v2"));
        let notes_copy = notes.skill_path.as_ref().unwrap().parent().unwrap().to_path_buf();
        std::fs::remove_dir_all(config.join("skills/notes")).unwrap();
        catalog_with_homes(home, &config, &codex, None).unwrap();
        assert!(!notes_copy.exists());
    }

    #[test]
    fn thingmaker_skills_are_listed_once_for_every_provider() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let text = "---\nname: big-thing\ndescription: Runs\n---\n";
        write(&super::super::thingmaker_skills_root(home).join("big-thing/SKILL.md"), text);
        write(&home.join(".agents/skills/big-thing/SKILL.md"), text);
        let found = catalog_with_homes(home, &config, &codex, None).unwrap();
        let own = found.entries.iter().find(|entry| entry.id == "skill:thingmaker:big-thing").unwrap();
        assert_eq!((own.group.as_str(), own.provider.as_deref(), own.available), ("thingmaker", Some("thingmaker"), true));
        let native = found.entries.iter().find(|entry| entry.id == "skill:agents-user:big-thing").unwrap();
        assert!(!native.available);
        assert_eq!(native.problem.as_deref(), Some("Same skill as ThingMaker's own copy"));
        assert!(super::setup(&super::root(home), "gemini-cli").is_err());
    }
}
