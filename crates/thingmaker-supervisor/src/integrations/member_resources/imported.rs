//! Imported skills: a copy of every skill a provider offers, local or from a
//! plugin, under `~/.thingmaker/imported-skills/<provider>/`. Once a provider
//! has been imported, the catalog lists those copies in place of the
//! provider's own skills, under the same ids, so saved selections keep working.
//!
//! Importing is something the person asks for (`sync`); listing the catalog
//! only reads. A catalog is built for one project and one account, and a copy
//! may be in use by a running session, so three rules hold:
//!
//! * A copy is never changed in place. A source that changed is copied into a
//!   new folder, named after its content, and the index is pointed at it.
//! * A copy stops being listed only when its source is gone from the disk, not
//!   because the catalog at hand (another project, another account) does not
//!   offer it.
//! * A folder the index no longer points at is removed only when no running
//!   ThingMaker has started a session with imported skills (`hold`).
//!
//! A plugin skill is copied with its whole plugin root, because skills reach
//! shared scripts through paths such as `../../scripts`. Project skills stay
//! native: they belong to one workspace, not to the provider.

use std::{collections::{BTreeMap, BTreeSet, HashMap}, fs::File, io::Write, path::{Component, Path, PathBuf}, sync::{LazyLock, Mutex}};

use serde::{Deserialize, Serialize};

use crate::{DesktopError, storage::content_hash};

use super::{ResourceEntry, bridge_skills, plain_name};

/// The providers whose skills can be imported.
pub const PROVIDERS: [&str; 3] = ["claude", "codex", "gemini"];
const INDEX: &str = "index.json";
/// Held by an import from start to end: two never run at once in one folder.
const SYNC_LOCK: &str = ".sync.lock";
/// Held, shared, by every ThingMaker that started a session with imported skills.
const IN_USE_LOCK: &str = ".in-use.lock";
/// A copy being written. Always inside the provider's folder, so it is on the
/// same disk as its final name and the rename that publishes it is atomic.
const TEMP_PREFIX: &str = ".tmp-";
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
    /// Skills of this catalog listed from their copies.
    pub skills: usize,
    /// Of those, the ones whose source has changed since it was copied.
    pub outdated: usize,
    /// Unix milliseconds of the last import.
    pub synced_at: Option<i64>,
    pub problem: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    synced_at: Option<i64>,
    /// Unit key → what was copied, and into which folder.
    units: BTreeMap<String, Unit>,
    /// Folders no unit points at any more, kept until nothing can be using them.
    retired: BTreeSet<String>,
    /// What the last import could not copy.
    problems: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Unit {
    source: PathBuf,
    fingerprint: String,
    /// The copy's folder inside the provider's folder.
    folder: String,
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

fn read_index(directory: &Path) -> Option<Index> {
    std::fs::read(directory.join(INDEX)).ok().map(|bytes| serde_json::from_slice(&bytes).unwrap_or_default())
}

/// Written beside the index and renamed over it: a catalog being built at the
/// same moment reads the old index or the new one, never half of either.
fn write_index(directory: &Path, index: &Index) -> std::io::Result<()> {
    let mut temp = tempfile::Builder::new().prefix(TEMP_PREFIX).tempfile_in(directory)?;
    temp.write_all(&serde_json::to_vec_pretty(index).map_err(std::io::Error::other)?)?;
    temp.persist(directory.join(INDEX)).map(|_| ()).map_err(|error| error.error)
}

/// A stable, readable name for one copied unit.
fn unit_key(entry: &ResourceEntry, unit_root: &Path) -> String {
    let label = match &entry.plugin {
        Some(_) => entry.group.trim_start_matches("plugin:").to_string(),
        None => format!("{}-{}", entry.group, unit_root.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()),
    };
    let label: String = label.chars().map(|c| if c.is_ascii_alphanumeric() || "@._-".contains(c) { c } else { '-' }).collect();
    format!("{}-{}", label.trim_start_matches('.'), &content_hash(unit_root.to_string_lossy().as_bytes())[..8])
}

/// The skills an import of `provider` copies, by unit: everything the
/// provider offers except a project's own skills.
fn candidates(entries: &[ResourceEntry], provider: &str) -> Vec<usize> {
    entries.iter().enumerate()
        .filter(|(_, entry)| entry.kind == "skill" && entry.provider.as_deref() == Some(provider) && entry.group != "project" && entry.skill_path.is_some()
            && entry.unit_root.as_ref().is_some_and(|unit| unit.file_name().is_some()))
        .map(|(position, _)| position).collect()
}

/// Walks a unit as an import sees it, skipped names left out. Only ordinary
/// files and folders are copied. A link can point outside the unit, and a
/// copy that followed it would hand its target on as an ordinary file: to a
/// local agent too, whose staging refuses linked files for that reason
/// (`copy_skill_files`). So a unit that holds one is not imported at all.
fn walk(directory: &Path, relative: &Path, visit: &mut dyn FnMut(&Path, &Path, &std::fs::Metadata) -> std::io::Result<()>) -> std::io::Result<()> {
    let mut entries = std::fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        if SKIPPED.iter().any(|skipped| name == *skipped) { continue }
        let path = entry.path();
        let relative = relative.join(&name);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            walk(&path, &relative, visit)?;
        } else if kind.is_file() {
            visit(&path, &relative, &entry.metadata()?)?;
        } else {
            return Err(std::io::Error::other(format!("{} is a link or a special file", relative.display())));
        }
    }
    Ok(())
}

/// Path, size and modification time of every file: it changes whenever a
/// plugin updates in place. Stops at the first byte over the limit.
fn fingerprint(unit_root: &Path) -> std::io::Result<String> {
    let mut hasher = blake3::Hasher::new();
    let mut total = 0;
    walk(unit_root, Path::new(""), &mut |_, relative, metadata| {
        let modified = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok()).map(|elapsed| elapsed.as_nanos()).unwrap_or_default();
        hasher.update(relative.to_string_lossy().as_bytes()).update(&[0]).update(&metadata.len().to_le_bytes()).update(&modified.to_le_bytes());
        total += metadata.len();
        if total > MAX_UNIT_BYTES { return Err(std::io::Error::other(format!("larger than {} MB", MAX_UNIT_BYTES / 1024 / 1024))) }
        Ok(())
    })?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Copies a unit into a new folder of the provider's directory, under its
/// source's own folder name, and only then gives it its final name. A failed
/// copy takes its temporary folder with it.
fn copy_unit(source: &Path, directory: &Path, folder: &str) -> std::io::Result<()> {
    let name = source.file_name().ok_or_else(|| std::io::Error::other("the source has no folder name"))?;
    let temp = tempfile::Builder::new().prefix(TEMP_PREFIX).tempdir_in(directory)?;
    let inside = temp.path().join(name);
    std::fs::create_dir_all(&inside)?;
    walk(source, Path::new(""), &mut |path, relative, _| {
        let target = inside.join(relative);
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::copy(path, target).map(|_| ())
    })?;
    let written = temp.keep();
    std::fs::rename(&written, directory.join(folder)).inspect_err(|_| { let _ = std::fs::remove_dir_all(&written); })
}

/// The ThingMaker processes that started a session with imported skills hold
/// this lock, shared, until they exit: this one's handles are kept here.
static IN_USE: LazyLock<Mutex<HashMap<PathBuf, File>>> = LazyLock::new(Default::default);

/// Called when a launch resolves to an imported copy. From then on no import,
/// here or in another ThingMaker, removes a folder a session may be reading.
pub(super) fn hold(root: &Path) {
    let mut held = IN_USE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if held.contains_key(root) { return }
    // Created by the first import; without it there are no copies to protect.
    if let Ok(file) = File::open(root.join(IN_USE_LOCK)) && file.lock_shared().is_ok() { held.insert(root.to_path_buf(), file); }
}

/// Whether no running ThingMaker has started a session with imported skills.
fn unused(root: &Path) -> bool {
    File::open(root.join(IN_USE_LOCK)).is_ok_and(|file| file.try_lock().is_ok())
}

/// Copies `provider`'s skills, as `entries` lists them before any import is
/// applied, and brings its copies up to date. See the module's three rules.
pub(super) fn sync(root: &Path, provider: &str, entries: &[ResourceEntry]) -> Result<(), DesktopError> {
    valid_provider(provider)?;
    let directory = root.join(provider);
    let failed = |error: std::io::Error| DesktopError::io(format!("Cannot import skills into {}: {error}", directory.display()));
    std::fs::create_dir_all(&directory).map_err(failed)?;
    if !root.join(IN_USE_LOCK).exists() { File::create(root.join(IN_USE_LOCK)).map_err(failed)?; }
    let lock = File::options().create(true).truncate(false).write(true).open(directory.join(SYNC_LOCK)).map_err(failed)?;
    if lock.try_lock().is_err() { return Err(DesktopError::conflict("These skills are already being imported. Try again when that import ends.")) }
    // Nothing else writes here while the lock is held: a temporary folder or
    // file found now was left by an import that did not end.
    for entry in std::fs::read_dir(&directory).map_err(failed)?.flatten() {
        if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX) {
            let _ = if entry.path().is_dir() { std::fs::remove_dir_all(entry.path()) } else { std::fs::remove_file(entry.path()) };
        }
    }
    let mut index = read_index(&directory).unwrap_or_default();
    let mut units = BTreeMap::new();
    for position in candidates(entries, provider) {
        let unit_root = entries[position].unit_root.clone().unwrap_or_default();
        units.entry(unit_key(&entries[position], &unit_root)).or_insert(unit_root);
    }
    let mut problems = Vec::new();
    for (key, source) in &units {
        let copied = match fingerprint(source) {
            Ok(fingerprint) => {
                let folder = format!("{key}-{}", &fingerprint[..12]);
                // The same content already has its folder: an unchanged unit,
                // or one that went back to an earlier state.
                let published = if directory.join(&folder).is_dir() { Ok(()) } else { copy_unit(source, &directory, &folder) };
                published.map(|_| Unit { source: source.clone(), fingerprint, folder })
            }
            Err(error) => Err(error),
        };
        match copied {
            Ok(unit) => { index.units.insert(key.clone(), unit); }
            // It stays as the provider has it; an earlier copy is not kept in its place.
            Err(error) => {
                problems.push(format!("{} was not imported: {error}", source.display()));
                index.units.remove(key);
            }
        }
    }
    // A unit this catalog does not offer may belong to another project or
    // account. It goes only when its source is gone from the disk.
    index.units.retain(|key, unit| units.contains_key(key) || unit.source.exists());
    // Every other folder here is a copy nothing points at any more.
    let current: BTreeSet<&str> = index.units.values().map(|unit| unit.folder.as_str()).collect();
    let mut retired = BTreeSet::new();
    for entry in std::fs::read_dir(&directory).map_err(failed)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('.') && plain_name(&name) && !current.contains(name.as_str()) && entry.file_type().is_ok_and(|kind| kind.is_dir()) { retired.insert(name); }
    }
    if unused(root) { retired.retain(|folder| std::fs::remove_dir_all(directory.join(folder)).is_err()); }
    index.retired = retired;
    index.problems = problems;
    index.synced_at = Some(now_millis());
    write_index(&directory, &index).map_err(failed)
}

/// Lists each imported provider's skills from their copies. Reads only.
pub(super) fn apply(entries: &mut [ResourceEntry], root: &Path) -> Vec<ImportStatus> {
    PROVIDERS.iter().map(|provider| apply_provider(entries, root, provider)).collect()
}

fn apply_provider(entries: &mut [ResourceEntry], root: &Path, provider: &str) -> ImportStatus {
    let directory = root.join(provider);
    let mut status = ImportStatus { provider: provider.into(), imported: false, skills: 0, outdated: 0, synced_at: None, problem: None };
    let Some(index) = read_index(&directory) else { return status };
    status.imported = true;
    status.synced_at = index.synced_at;
    status.problem = (!index.problems.is_empty()).then(|| index.problems.join("\n"));
    for position in candidates(entries, provider) {
        let entry = &mut entries[position];
        let (Some(unit_root), Some(skill_path)) = (entry.unit_root.clone(), entry.skill_path.clone()) else { continue };
        let Some(unit) = index.units.get(&unit_key(entry, &unit_root)).filter(|unit| plain_name(&unit.folder)) else { continue };
        let (Some(name), Ok(relative)) = (unit_root.file_name(), skill_path.strip_prefix(&unit_root)) else { continue };
        // A plugin can list a skill outside its own root; that one stays native.
        if !relative.components().all(|part| matches!(part, Component::Normal(_))) { continue }
        let copy_root = directory.join(&unit.folder).join(name);
        let copy = copy_root.join(relative);
        let Ok(bytes) = std::fs::read(&copy) else { continue };
        let hash = content_hash(&bytes);
        if hash != entry.skill_hash { status.outdated += 1; }
        entry.revision = format!("{}:{hash}", copy.display());
        entry.skill_hash = hash;
        entry.skill_path = Some(copy);
        entry.unit_root = Some(copy_root);
        bridge_skills(std::slice::from_mut(entry), &format!("imported-skills/{provider}"));
        status.skills += 1;
    }
    status
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use serde_json::json;

    use super::super::{ResourceCatalog, ResourceSelection, catalog_with_homes, import_skills, resolve};

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Every file under `directory` with its content: what an import left on disk.
    fn tree(directory: &Path) -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        let mut pending = vec![directory.to_path_buf()];
        while let Some(next) = pending.pop() {
            for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
                if entry.file_type().unwrap().is_dir() { pending.push(entry.path()); } else { found.push((entry.path(), std::fs::read_to_string(entry.path()).unwrap_or_default())); }
            }
        }
        found.sort();
        found
    }

    fn status<'a>(catalog: &'a ResourceCatalog, provider: &str) -> &'a super::ImportStatus {
        catalog.imports.iter().find(|status| status.provider == provider).unwrap()
    }

    fn skill_file(catalog: &ResourceCatalog, id: &str) -> PathBuf {
        catalog.entries.iter().find(|entry| entry.id == id).unwrap_or_else(|| panic!("{id} missing")).skill_path.clone().unwrap()
    }

    /// A restart of ThingMaker, as far as the copies are concerned.
    fn restart(root: &Path) {
        super::IN_USE.lock().unwrap().remove(root);
    }

    #[test]
    fn listing_only_reads_and_importing_is_asked_for() {
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
        assert!(!status(&native, "claude").imported);
        assert!(native.entries.iter().any(|entry| entry.source == ".claude/skills"));
        assert!(!root.exists(), "listing creates nothing");

        let imported = import_skills(home, &config, &codex, None, "claude").unwrap();
        let state = status(&imported, "claude");
        assert!(state.imported && state.synced_at.is_some() && state.problem.is_none(), "{state:?}");
        assert_eq!((state.skills, state.outdated), (2, 0));
        let draw = imported.entries.iter().find(|entry| entry.id == "skill:art@market:draw").unwrap();
        assert_eq!(draw.source, "imported-skills/claude");
        assert!(draw.bridge_skill && draw.available);
        let copy = draw.skill_path.as_ref().unwrap();
        assert!(copy.starts_with(root.join("claude")));
        let plugin_copy = copy.parent().unwrap().parent().unwrap().parent().unwrap();
        assert_eq!(plugin_copy.file_name().unwrap(), "plugin", "a copy keeps its source's folder name");
        assert!(plugin_copy.join("scripts/draw.sh").is_file(), "the plugin root comes along");
        assert!(!plugin_copy.join("node_modules").exists());

        // The same ids resolve to the copies, so saved selections keep working.
        let selection = ResourceSelection { skills: vec!["skill:user:notes".into()], ..Default::default() };
        let resolved = resolve(&imported, &selection, &Default::default()).unwrap();
        assert!(resolved.skill_sources[0].path.starts_with(root.join("claude").canonicalize().unwrap()));
        assert_eq!(resolved.skill_sources[0].path.parent().unwrap().file_name().unwrap(), "notes");

        // A source edit changes nothing on disk until the person asks again:
        // listing says the copy is behind, and still lists it.
        let before = tree(&root);
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\nv2 is longer");
        let listed = catalog_with_homes(home, &config, &codex, None).unwrap();
        assert_eq!(tree(&root), before, "listing writes nothing");
        assert_eq!((status(&listed, "claude").skills, status(&listed, "claude").outdated), (2, 1));
        assert!(std::fs::read_to_string(skill_file(&listed, "skill:user:notes")).unwrap().ends_with("v1"));
        let stale = resolve(&listed, &selection, &Default::default()).unwrap().revision;

        let refreshed = import_skills(home, &config, &codex, None, "claude").unwrap();
        assert_eq!(status(&refreshed, "claude").outdated, 0);
        assert!(std::fs::read_to_string(skill_file(&refreshed, "skill:user:notes")).unwrap().ends_with("v2 is longer"));
        assert_ne!(resolve(&refreshed, &selection, &Default::default()).unwrap().revision, stale, "a worker started on the old copy is not taken for one on the new");
        assert!(import_skills(home, &config, &codex, None, "gemini-cli").is_err());
    }

    #[test]
    fn a_copy_a_session_may_be_reading_is_neither_changed_nor_removed() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\nv1");
        write(&config.join("skills/notes/helper.txt"), "help v1");
        let root = super::root(home);
        let imported = import_skills(home, &config, &codex, None, "claude").unwrap();
        // A session starts with the copy.
        let selection = ResourceSelection { skills: vec!["skill:user:notes".into()], ..Default::default() };
        let in_use = resolve(&imported, &selection, &Default::default()).unwrap().skill_sources[0].path.clone();

        // The source changes and is imported again; then it is deleted and imported again.
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\nsecond");
        let refreshed = import_skills(home, &config, &codex, None, "claude").unwrap();
        let second = skill_file(&refreshed, "skill:user:notes");
        assert_ne!(second.canonicalize().unwrap(), in_use, "the change went into a new folder");
        assert!(std::fs::read_to_string(&in_use).unwrap().ends_with("v1"));
        assert_eq!(std::fs::read_to_string(in_use.with_file_name("helper.txt")).unwrap(), "help v1");
        std::fs::remove_dir_all(config.join("skills/notes")).unwrap();
        let gone = import_skills(home, &config, &codex, None, "claude").unwrap();
        assert!(gone.entries.iter().all(|entry| entry.id != "skill:user:notes"));
        assert!(in_use.is_file() && second.is_file(), "nothing a running session may read is removed");

        // Once no ThingMaker that started such a session is running, the next import clears them.
        restart(&root);
        import_skills(home, &config, &codex, None, "claude").unwrap();
        assert!(!in_use.exists() && !second.exists());
        assert!(tree(&root.join("claude")).iter().all(|(path, _)| path.file_name().unwrap().to_string_lossy().starts_with('.') || path.ends_with("index.json")), "{:?}", tree(&root.join("claude")));
    }

    #[test]
    fn copies_of_another_project_or_account_are_left_alone() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let other_account = temp.path().join(".claude-work");
        let project = temp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\n");
        write(&other_account.join("skills/report/SKILL.md"), "---\nname: report\ndescription: Reports\n---\n");
        // A plugin installed for one project only.
        let plugin = temp.path().join("plugin");
        write(&plugin.join(".claude-plugin/plugin.json"), r#"{"name":"art"}"#);
        write(&plugin.join("skills/draw/SKILL.md"), "---\nname: draw\ndescription: Draws\n---\n");
        write(&config.join("plugins/installed_plugins.json"), &json!({"version":2,"plugins":{"art@market":[{"scope":"project","projectPath":project,"installPath":plugin}]}}).to_string());
        write(&config.join("settings.json"), r#"{"enabledPlugins":{"art@market":true}}"#);
        let root = super::root(home);
        let imported = |catalog: &ResourceCatalog, id: &str| catalog.entries.iter().find(|entry| entry.id == id).is_some_and(|entry| entry.source == "imported-skills/claude");

        let in_project = import_skills(home, &config, &codex, Some(&project), "claude").unwrap();
        assert!(imported(&in_project, "skill:art@market:draw") && imported(&in_project, "skill:user:notes"));
        let draw = skill_file(&in_project, "skill:art@market:draw");
        // Imported again with no project open, and from another account: the
        // project's plugin and the first account's skill are not on offer there.
        let elsewhere = import_skills(home, &config, &codex, None, "claude").unwrap();
        assert!(elsewhere.entries.iter().all(|entry| entry.id != "skill:art@market:draw"));
        let other = import_skills(home, &other_account, &codex, None, "claude").unwrap();
        assert!(imported(&other, "skill:user:report"));
        restart(&root);
        import_skills(home, &other_account, &codex, None, "claude").unwrap();
        assert!(draw.is_file(), "a copy is not removed for being out of sight");
        let back = catalog_with_homes(home, &config, &codex, Some(&project)).unwrap();
        assert!(imported(&back, "skill:art@market:draw") && imported(&back, "skill:user:notes"));
        assert_eq!(skill_file(&back, "skill:art@market:draw"), draw);

        // Its source uninstalled, it goes with the next import, whoever asks.
        std::fs::remove_dir_all(&plugin).unwrap();
        import_skills(home, &other_account, &codex, None, "claude").unwrap();
        assert!(!draw.exists());
    }

    #[test]
    fn two_imports_never_interleave_and_a_broken_one_leaves_nothing_behind() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\n");
        let directory = super::root(home).join("claude");
        import_skills(home, &config, &codex, None, "claude").unwrap();
        let before = tree(&directory);

        // Another import holds the folder: this one changes nothing and says why.
        write(&config.join("skills/notes/SKILL.md"), "---\nname: notes\ndescription: Notes\n---\nchanged meanwhile");
        let running = std::fs::File::options().write(true).open(directory.join(super::SYNC_LOCK)).unwrap();
        running.lock().unwrap();
        let refused = import_skills(home, &config, &codex, None, "claude").unwrap_err();
        assert!(refused.message.contains("already being imported"), "{}", refused.message);
        assert_eq!(tree(&directory), before);
        drop(running);

        // What an import that died left half-written is swept by the next one.
        write(&directory.join(".tmp-dead/notes/SKILL.md"), "half");
        write(&directory.join(".tmp-dead-index"), "{");
        // A unit that cannot be copied leaves no temporary folder, and the others are imported.
        write(&config.join("skills/huge/SKILL.md"), "---\nname: huge\ndescription: Huge\n---\n");
        let big = std::fs::File::create(config.join("skills/huge/data.bin")).unwrap();
        big.set_len(super::MAX_UNIT_BYTES + 1).unwrap();
        let imported = import_skills(home, &config, &codex, None, "claude").unwrap();
        let state = status(&imported, "claude");
        assert!(state.problem.as_deref().is_some_and(|problem| problem.contains("huge") && problem.contains("larger than 200 MB")), "{state:?}");
        assert_eq!(imported.entries.iter().find(|entry| entry.id == "skill:user:huge").unwrap().source, ".claude/skills", "it stays as the provider has it");
        assert!(std::fs::read_to_string(skill_file(&imported, "skill:user:notes")).unwrap().ends_with("changed meanwhile"));
        let names: Vec<String> = std::fs::read_dir(&directory).unwrap().flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        assert!(names.iter().all(|name| !name.starts_with(super::TEMP_PREFIX)), "{names:?}");
        // The index is whole after every import, and a damaged one is rebuilt from the sources.
        std::fs::write(directory.join(super::INDEX), "{ not json").unwrap();
        let rebuilt = import_skills(home, &config, &codex, None, "claude").unwrap();
        assert_eq!(status(&rebuilt, "claude").skills, 1);
    }

    #[test]
    fn a_skill_turned_off_in_claude_stays_off_through_its_copy() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        // Claude Code names a skill without a frontmatter name after its folder,
        // and its settings turn skills off by that folder name.
        write(&config.join("skills/plain/SKILL.md"), "---\ndescription: No name of its own\n---\n");
        write(&config.join("skills/renamed/SKILL.md"), "---\nname: other-name\ndescription: Named apart from its folder\n---\n");
        write(&config.join("skills/kept/SKILL.md"), "---\ndescription: Left on\n---\n");
        write(&config.join("settings.json"), r#"{"skillOverrides":{"plain":"off","renamed":"user-invocable-only"}}"#);
        let off = |catalog: &ResourceCatalog| ["plain", "renamed", "kept"].map(|folder| {
            let entry = catalog.entries.iter().find(|entry| entry.id == format!("skill:user:{folder}")).unwrap();
            (entry.name.clone(), entry.available, entry.problem.clone().unwrap_or_default())
        });
        let native = off(&catalog_with_homes(home, &config, &codex, None).unwrap());
        let imported = import_skills(home, &config, &codex, None, "claude").unwrap();
        assert_eq!(status(&imported, "claude").skills, 3);
        let turned_off = "Skill disabled for model invocation in Claude settings".to_string();
        assert_eq!(off(&imported), [("plain".to_string(), false, turned_off.clone()), ("other-name".to_string(), false, turned_off), ("kept".to_string(), true, String::new())]);
        assert_eq!(off(&imported).map(|(_, available, problem)| (available, problem)), native.map(|(_, available, problem)| (available, problem)), "importing changes nothing about what is on");
        // The copy is the folder Claude Code and Codex would have read, name and all.
        assert_eq!(skill_file(&imported, "skill:user:kept").parent().unwrap().file_name().unwrap(), "kept");
        let kept = ResourceSelection { skills: vec!["skill:user:kept".into()], ..Default::default() };
        assert!(resolve(&imported, &kept, &Default::default()).unwrap().skills[0].ends_with(":kept"));
        let plain = ResourceSelection { skills: vec!["skill:user:plain".into()], ..Default::default() };
        assert!(resolve(&imported, &plain, &Default::default()).is_err(), "nor can a saved selection start it");
    }

    #[cfg(unix)]
    #[test]
    fn nothing_linked_from_outside_a_skill_is_copied() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let skill = |name: &str| format!("---\nname: {name}\ndescription: {name}\n---\n");
        write(&temp.path().join("private/key.txt"), "not for agents");
        write(&temp.path().join("private/more/notes.txt"), "nor this");
        // One skill links a file outside its folder, another a folder.
        write(&config.join("skills/file-link/SKILL.md"), &skill("file-link"));
        std::os::unix::fs::symlink(temp.path().join("private/key.txt"), config.join("skills/file-link/key.txt")).unwrap();
        write(&config.join("skills/folder-link/SKILL.md"), &skill("folder-link"));
        std::os::unix::fs::symlink(temp.path().join("private/more"), config.join("skills/folder-link/more")).unwrap();
        // A skill folder that is itself a link is where the person keeps that skill.
        write(&temp.path().join("kept-elsewhere/shared/SKILL.md"), &skill("shared"));
        write(&temp.path().join("kept-elsewhere/shared/helper.txt"), "help");
        std::os::unix::fs::symlink(temp.path().join("kept-elsewhere/shared"), config.join("skills/shared")).unwrap();

        let imported = import_skills(home, &config, &codex, None, "claude").unwrap();
        let state = status(&imported, "claude");
        let problem = state.problem.clone().unwrap_or_default();
        assert!(problem.contains("key.txt is a link") && problem.contains("more is a link"), "{problem}");
        assert_eq!(state.skills, 1);
        let source = |id: &str| imported.entries.iter().find(|entry| entry.id == id).unwrap().source.clone();
        assert_eq!((source("skill:user:file-link").as_str(), source("skill:user:folder-link").as_str()), (".claude/skills", ".claude/skills"), "they stay as the provider has them");
        assert_eq!(source("skill:user:shared"), "imported-skills/claude");
        assert_eq!(std::fs::read_to_string(skill_file(&imported, "skill:user:shared").with_file_name("helper.txt")).unwrap(), "help");
        let copied = tree(&super::root(home));
        assert!(copied.iter().all(|(path, text)| !text.contains("not for agents") && !text.contains("nor this") && !path.ends_with("key.txt")), "{copied:?}");

        // So a local agent is refused the linked skill exactly as before, and gets the plain one.
        let local = imported.local_only();
        let pick = |id: &str| ResourceSelection { skills: vec![id.into()], ..Default::default() };
        assert!(resolve(&local, &pick("skill:user:file-link"), &Default::default()).is_err());
        assert!(resolve(&local, &pick("skill:user:shared"), &Default::default()).is_ok());
    }

    #[test]
    fn a_native_copy_of_a_thingmaker_skill_stays_selectable() {
        let temp = tempfile::tempdir().unwrap();
        let (home, config, codex) = (temp.path(), temp.path().join(".claude"), temp.path().join(".codex"));
        let text = "---\nname: big-thing\ndescription: Runs\n---\n";
        write(&super::super::thingmaker_skills_root(home).join("big-thing/SKILL.md"), text);
        write(&home.join(".agents/skills/big-thing/SKILL.md"), text);
        let found = catalog_with_homes(home, &config, &codex, None).unwrap();
        let own = found.entries.iter().find(|entry| entry.id == "skill:thingmaker:big-thing").unwrap();
        assert_eq!((own.group.as_str(), own.provider.as_deref(), own.available), ("thingmaker", Some("thingmaker"), true));
        assert_eq!(own.duplicate_of, None);
        // A team saved before ThingMaker had its own skills selected the native copy.
        let native = found.entries.iter().find(|entry| entry.id == "skill:agents-user:big-thing").unwrap();
        assert!(native.available && native.problem.is_none());
        assert_eq!(native.duplicate_of.as_deref(), Some("skill:thingmaker:big-thing"));
        let saved = ResourceSelection { skills: vec!["skill:agents-user:big-thing".into()], ..Default::default() };
        let resolved = resolve(&found, &saved, &Default::default()).unwrap();
        assert!(resolved.skill_sources[0].path.starts_with(home.join(".agents/skills").canonicalize().unwrap()));
        // Given both, the member loads the skill once.
        let both = ResourceSelection { skills: vec!["skill:agents-user:big-thing".into(), "skill:thingmaker:big-thing".into()], ..Default::default() };
        let resolved = resolve(&found, &both, &Default::default()).unwrap();
        assert_eq!(resolved.skill_sources.len(), 1);
        assert!(resolved.skill_sources[0].path.starts_with(home.join(".thingmaker/skills").canonicalize().unwrap()));
        // An edited native copy is a skill of its own.
        write(&home.join(".agents/skills/big-thing/SKILL.md"), "---\nname: big-thing\ndescription: Mine\n---\n");
        let found = catalog_with_homes(home, &config, &codex, None).unwrap();
        assert_eq!(found.entries.iter().find(|entry| entry.id == "skill:agents-user:big-thing").unwrap().duplicate_of, None);
    }
}
