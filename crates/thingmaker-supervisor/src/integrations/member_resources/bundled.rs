//! Skills shipped inside ThingMaker, installed in its own skills folder so
//! every agent's selector offers them. The folder is also the person's: they
//! can edit or remove what is there. So a shipped skill is written only when
//! it was never installed, and updated only while the installed file is still
//! the one ThingMaker wrote. `.bundled.json` remembers what that was.

use std::{collections::BTreeMap, io::Write, path::Path};

use crate::storage::content_hash;

const RECORD: &str = ".bundled.json";

/// Installs or updates each `(folder name, SKILL.md text)` under `root`.
/// Returns the names it wrote. A skill the person edited or removed is left
/// as they left it.
pub fn install(root: &Path, skills: &[(&str, &str)]) -> std::io::Result<Vec<String>> {
    let mut record: BTreeMap<String, String> = std::fs::read(root.join(RECORD)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
    let before = record.clone();
    let mut written = Vec::new();
    for (name, text) in skills {
        let file = root.join(name).join("SKILL.md");
        let shipped = content_hash(text.as_bytes());
        let installed = std::fs::read(&file).ok().map(|bytes| content_hash(&bytes));
        let ours = record.get(*name);
        let write = match &installed {
            Some(installed) if *installed == shipped => false,
            // Still the file an earlier version wrote: this version's replaces it.
            Some(installed) => ours == Some(installed),
            // Never installed. Once it was, a missing file is the person's removal.
            None => ours.is_none(),
        };
        if write {
            std::fs::create_dir_all(root.join(name))?;
            std::fs::write(&file, text)?;
            written.push(name.to_string());
        }
        if write || installed.as_ref() == Some(&shipped) { record.insert(name.to_string(), shipped); }
    }
    if record != before {
        let mut temp = tempfile::NamedTempFile::new_in(root)?;
        temp.write_all(&serde_json::to_vec_pretty(&record).map_err(std::io::Error::other)?)?;
        temp.persist(root.join(RECORD)).map_err(|error| error.error)?;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::install;

    #[test]
    fn a_shipped_skill_is_updated_only_while_it_is_still_the_shipped_one() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skills");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("guide/SKILL.md");
        let read = || std::fs::read_to_string(&file).unwrap();

        assert_eq!(install(&root, &[("guide", "v1")]).unwrap(), ["guide"]);
        assert_eq!(read(), "v1");
        assert!(install(&root, &[("guide", "v1")]).unwrap().is_empty(), "nothing to do twice");
        // An update of the application replaces the file it wrote itself.
        assert_eq!(install(&root, &[("guide", "v2")]).unwrap(), ["guide"]);
        assert_eq!(read(), "v2");

        // The person's edit survives this version and the next ones.
        std::fs::write(&file, "mine").unwrap();
        assert!(install(&root, &[("guide", "v2")]).unwrap().is_empty());
        assert!(install(&root, &[("guide", "v3")]).unwrap().is_empty());
        assert_eq!(read(), "mine");
        // Put back as shipped by hand, it follows updates again.
        std::fs::write(&file, "v3").unwrap();
        assert!(install(&root, &[("guide", "v3")]).unwrap().is_empty());
        assert_eq!(install(&root, &[("guide", "v4")]).unwrap(), ["guide"]);

        // Removed by the person, it is not brought back.
        std::fs::remove_dir_all(root.join("guide")).unwrap();
        assert!(install(&root, &[("guide", "v5")]).unwrap().is_empty());
        assert!(!file.exists());
    }

    #[test]
    fn a_skill_already_there_under_the_same_name_is_the_persons() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("skills");
        std::fs::create_dir_all(root.join("guide")).unwrap();
        std::fs::write(root.join("guide/SKILL.md"), "written by hand").unwrap();
        assert!(install(&root, &[("guide", "v1")]).unwrap().is_empty());
        assert_eq!(std::fs::read_to_string(root.join("guide/SKILL.md")).unwrap(), "written by hand");
    }
}
