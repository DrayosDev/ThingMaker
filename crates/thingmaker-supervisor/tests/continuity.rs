use thingmaker_supervisor::{agents::Provider, continuity::{Identity, Limits, Store, MAX_BYTES, MAX_CHARS}};
use tempfile::TempDir;

fn identity(root: &TempDir, session: &str) -> Identity { Identity::new(root.path(), Provider::Codex, "default".into(), session.into()).unwrap() }

fn files(data: &TempDir) -> usize {
    std::fs::read_dir(data.path().join("continuity")).unwrap().flatten().filter(|entry| entry.path().extension().is_some_and(|extension| extension == "md")).count()
}

#[test]
fn a_chain_of_hand_offs_is_followed_link_by_link_and_nothing_is_copied_forward() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b"); let c = identity(&root, "c");
    let store = Store::open(data.path()).unwrap();
    let first = store.save(&a, "Full A history").unwrap();
    assert!(store.bound(&a).unwrap().is_none());
    assert!(first.parent.is_none());
    assert_eq!((first.chars, first.bytes, first.provider), (14, 14, Provider::Codex));
    store.attach(&a, &b, &first.id).unwrap(); drop(store);
    // The application restarts between the hand-offs.
    let store = Store::open(data.path()).unwrap();
    assert_eq!(store.bound(&b).unwrap().unwrap().id, first.id);
    let second = store.save(&b, "Full B history").unwrap();
    assert_eq!(second.parent.as_deref(), Some(first.id.as_str()));
    store.attach(&b, &c, &second.id).unwrap(); drop(store);
    let store = Store::open(data.path()).unwrap();
    // Each file holds its own session's words and no one else's.
    assert_eq!(std::fs::read_to_string(&first.path).unwrap(), "Full A history");
    assert_eq!(std::fs::read_to_string(&second.path).unwrap(), "Full B history");
    let chain: Vec<String> = store.chain(&c).unwrap().into_iter().map(|archive| archive.id).collect();
    assert_eq!(chain, [second.id.clone(), first.id.clone()]);
    assert_eq!(store.read(&c, &second.id, None, None).unwrap().text, "Full B history");
    assert_eq!(store.read(&c, &first.id, None, None).unwrap().text, "Full A history");
    // The leader's tool sees the same chain, and b sees only what it was handed.
    let listed = store.tool_read(&c, true, None, None, None).unwrap();
    assert_eq!(listed["archive"]["id"], second.id);
    assert_eq!(listed["earlier"][0]["id"], first.id);
    assert_eq!(store.tool_read(&c, true, Some(&first.id), None, None).unwrap()["text"], "Full A history");
    assert!(store.tool_read(&b, true, Some(&second.id), None, None).is_err());
    assert_eq!(store.tool_read(&b, true, Some(&first.id), None, None).unwrap()["text"], "Full A history");
    assert_eq!(files(&data), 2);
}

#[test]
fn unicode_pages_and_bounds() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap(); let a = identity(&root,"a");
    let store = Store::open(data.path()).unwrap(); let archive = store.save(&a, "A😀é中Z").unwrap();
    assert_eq!(archive.chars, 5);
    let p = store.read(&a, &archive.id, Some(1), Some(2)).unwrap();
    assert_eq!(p.text,"😀é"); assert_eq!(p.total_chars,5); assert_eq!(p.next_offset,Some(3));
    assert_eq!(store.read(&a,&archive.id,Some(5),None).unwrap().next_offset,None);
    for (offset,limit) in [(6,1),(0,0),(0,MAX_CHARS+1)] { assert!(store.read(&a,&archive.id,Some(offset),Some(limit)).is_err()); }
}

#[test]
fn scoped_access_workers_and_unbound_owner_denied_by_tool() {
    let data=TempDir::new().unwrap(); let root=TempDir::new().unwrap(); let other=TempDir::new().unwrap();
    let a=identity(&root,"a"); let b=identity(&root,"b"); let stranger=identity(&root,"stranger"); let foreign=identity(&other,"b");
    let store=Store::open(data.path()).unwrap(); let archive=store.save(&a,"private").unwrap();
    assert!(store.read(&stranger,&archive.id,None,None).is_err());
    assert!(store.attach(&a,&foreign,&archive.id).is_err());
    assert!(store.attach(&stranger,&b,&archive.id).is_err());
    assert!(store.tool_read(&a,true,Some(&archive.id),None,None).is_err());
    store.attach(&a,&b,&archive.id).unwrap();
    assert!(store.tool_read(&b,true,Some(&archive.id),None,None).is_ok());
    assert!(store.tool_read(&b,false,Some(&archive.id),None,None).is_err());
    assert!(store.tool_read(&b,false,None,None,None).is_err());
    assert!(store.tool_read(&foreign,true,Some(&archive.id),None,None).is_err());
    assert!(store.tool_read(&stranger,true,None,None,None).unwrap()["archive"].is_null());
    let other_provider=Identity::new(root.path(),Provider::Claude,"default".into(),"b".into()).unwrap();
    assert!(store.read(&other_provider,&archive.id,None,None).is_err());
}

#[test]
fn an_oversized_conversation_is_refused_as_a_limit_and_invalid_ids_change_nothing() {
    let data=TempDir::new().unwrap(); let root=TempDir::new().unwrap(); let a=identity(&root,"a"); let b=identity(&root,"b");
    let store=Store::open(data.path()).unwrap();
    // The caller tells this refusal apart: it hands over the excerpt alone.
    let refused = store.save(&a,&"x".repeat(MAX_BYTES+1)).unwrap_err();
    assert_eq!(serde_json::to_value(&refused).unwrap()["code"], "LIMIT_EXCEEDED");
    assert_eq!(files(&data), 0);
    let archive=store.save(&a,&"x".repeat(MAX_BYTES)).unwrap(); store.attach(&a,&b,&archive.id).unwrap();
    // The next session's own archive stands by itself: what it inherited no longer counts against it.
    assert!(store.save(&b,"new").is_ok()); assert_eq!(store.bound(&b).unwrap().unwrap().id,archive.id);
    for id in ["", "../index.sqlite3","../../secret","not-a-uuid", "C:\\secret.md", "00000000-0000-0000-0000-000000000000"] {
        assert!(store.read(&a,id,None,None).is_err()); assert!(store.attach(&a,&b,id).is_err());
        assert!(store.tool_read(&b,true,Some(id),None,None).is_err());
    }
}

#[test]
fn accounts_are_isolated_and_explicit_transfer_requires_the_owner() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let source = identity(&root, "same-provider-session-id");
    let target = Identity::new(root.path(), Provider::Codex, "second-account".into(), "same-provider-session-id".into()).unwrap();
    let stranger = Identity::new(root.path(), Provider::Codex, "third-account".into(), "same-provider-session-id".into()).unwrap();
    let store = Store::open(data.path()).unwrap();
    let archive = store.save(&source, "only source owns this").unwrap();
    assert!(store.read(&target, &archive.id, None, None).is_err());
    assert!(store.tool_read(&target, true, Some(&archive.id), None, None).is_err());
    assert!(store.attach(&target, &stranger, &archive.id).is_err());
    store.attach(&source, &target, &archive.id).unwrap();
    assert!(store.read(&target, &archive.id, None, None).is_ok());
    assert!(store.tool_read(&target, true, Some(&archive.id), None, None).is_ok());
    assert!(store.read(&stranger, &archive.id, None, None).is_err());
    assert!(store.attach(&target, &stranger, &archive.id).is_err());
    drop(store);
    let store = Store::open(data.path()).unwrap();
    assert_eq!(store.bound(&target).unwrap().unwrap().id, archive.id);
    assert!(store.bound(&stranger).unwrap().is_none());
    let normalized = Identity::new(root.path(), Provider::Codex, String::new(), "same-provider-session-id".into()).unwrap();
    assert_eq!(normalized, source);
    assert!(store.read(&normalized, &archive.id, None, None).is_ok());
    assert!(Identity::new(root.path(), Provider::Codex, "default".into(), String::new()).is_err());
}

#[test]
fn bindings_are_idempotent_but_cannot_be_reassigned_or_self_attached() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b");
    let store = Store::open(data.path()).unwrap();
    let first = store.save(&a, "first").unwrap();
    let second = store.save(&a, "second").unwrap();
    assert!(store.attach(&a, &a, &first.id).is_err());
    assert!(store.bound(&a).unwrap().is_none());
    store.attach(&a, &b, &first.id).unwrap();
    store.attach(&a, &b, &first.id).unwrap();
    assert!(store.attach(&a, &b, &second.id).is_err());
    assert_eq!(store.bound(&b).unwrap().unwrap().id, first.id);
    assert!(store.tool_read(&b, true, Some(&second.id), None, None).is_err());
    drop(store);
    let store = Store::open(data.path()).unwrap();
    assert!(store.attach(&a, &b, &second.id).is_err());
    assert_eq!(store.read(&b, &first.id, None, None).unwrap().text, "first");
}

#[test]
fn same_size_tamper_is_rejected_for_read_and_attach() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b"); let c = identity(&root, "c");
    let store = Store::open(data.path()).unwrap();
    let archive = store.save(&a, "original").unwrap();
    store.attach(&a, &b, &archive.id).unwrap();
    std::fs::write(&archive.path, "modified").unwrap();
    assert_eq!(std::fs::metadata(&archive.path).unwrap().len(), archive.bytes);
    assert!(store.read(&a, &archive.id, None, None).is_err());
    assert!(store.tool_read(&b, true, Some(&archive.id), None, None).is_err());
    assert!(store.attach(&a, &c, &archive.id).is_err());
    assert!(store.bound(&c).unwrap().is_none());
    assert_eq!(store.bound(&b).unwrap().unwrap().id, archive.id);
    drop(store);
    assert!(Store::open(data.path()).unwrap().read(&a, &archive.id, None, None).is_err());
}

#[test]
fn an_index_from_a_development_build_is_replaced_with_its_files_not_trusted() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a");
    let dir = data.path().join("continuity"); std::fs::create_dir(&dir).unwrap();
    let db = rusqlite::Connection::open(dir.join("index.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE archives(id TEXT PRIMARY KEY,owner TEXT NOT NULL,root TEXT NOT NULL,bytes INTEGER NOT NULL,created INTEGER NOT NULL); CREATE TABLE bindings(target TEXT PRIMARY KEY,archive TEXT NOT NULL);").unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let canonical = root.path().canonicalize().unwrap().to_string_lossy().into_owned();
    let owner = serde_json::to_string(&(&canonical, "codex", "default", "a")).unwrap();
    db.execute("INSERT INTO archives VALUES(?1,?2,?3,6,0)", rusqlite::params![id, owner, canonical]).unwrap();
    std::fs::write(dir.join(format!("{id}.md")), "legacy").unwrap(); drop(db);
    let store = Store::open(data.path()).unwrap();
    assert!(store.read(&a, &id, None, None).is_err());
    assert_eq!(files(&data), 0, "an archive with no integrity record is not left lying there");
    let fresh = store.save(&a, "fresh").unwrap();
    assert_eq!(store.read(&a, &fresh.id, None, None).unwrap().text, "fresh");
    drop(store);
    // Reopening the current shape keeps what it holds.
    assert!(Store::open(data.path()).unwrap().read(&a, &fresh.id, None, None).is_ok());
    assert_eq!(files(&data), 1);
}

#[test]
fn the_reader_is_pointed_at_pages_not_told_to_load_everything_or_where_the_files_are() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b");
    let store = Store::open(data.path()).unwrap(); let text = "\u{1f600}".repeat(12_001);
    let archive = store.save(&a, &text).unwrap(); store.attach(&a, &b, &archive.id).unwrap();
    let discovery = store.tool_read(&b, true, None, None, None).unwrap();
    let note = discovery["note"].as_str().unwrap();
    assert!(note.contains("Do not read it all by default") && note.contains("offset and limit"), "{note}");
    assert!(!note.contains("entire archive") && !note.contains("until nextOffset is null"), "{note}");
    assert_eq!(discovery["archive"]["chars"], 12_001);
    // The tool never says where the file is: the folder holds other sessions' archives.
    let first = store.tool_read(&b, true, Some(&archive.id), None, None).unwrap();
    for answer in [&discovery, &first] {
        assert!(answer["archive"].get("path").is_none(), "{answer}");
        assert!(!answer.to_string().contains("continuity"), "no path in what the agent is told");
    }
    // Paging still reaches everything, for a reader that does need it all.
    let mut offset = Some(0); let mut restored = String::new();
    while let Some(start) = offset {
        let page = store.tool_read(&b, true, Some(&archive.id), Some(start), None).unwrap();
        restored.push_str(page["text"].as_str().unwrap());
        offset = page["nextOffset"].as_u64().map(|n| n as usize);
    }
    assert_eq!(restored, text);
}

#[test]
fn deleting_a_sessions_archives_takes_its_words_from_every_later_reader() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap(); let other = TempDir::new().unwrap();
    let a = identity(&root, "a"); let b = identity(&root, "b"); let c = identity(&root, "c");
    // The same conversation id under another account is the same session to delete.
    let a_elsewhere = Identity::new(root.path(), Provider::Codex, "work".into(), "a".into()).unwrap();
    let theirs = identity(&other, "a");
    let store = Store::open(data.path()).unwrap();
    let first = store.save(&a, "secret of A").unwrap();
    let moved = store.save(&a_elsewhere, "more of A").unwrap();
    let kept = store.save(&theirs, "another project").unwrap();
    store.attach(&a, &b, &first.id).unwrap();
    let second = store.save(&b, "B").unwrap();
    store.attach(&b, &c, &second.id).unwrap();
    assert_eq!(store.chain(&c).unwrap().len(), 2);

    // Session a is wiped, by the workspace root as the desktop records it.
    assert_eq!(store.delete_sessions(root.path(), &[(Provider::Codex, "a".into()), (Provider::Claude, "b".into())]).unwrap(), 2);
    assert!(!std::path::Path::new(&first.path).exists() && !std::path::Path::new(&moved.path).exists());
    assert!(store.bound(&b).unwrap().is_none(), "nothing is bound to a deleted archive");
    let chain = store.chain(&c).unwrap();
    assert_eq!(chain.len(), 1);
    assert!(chain[0].parent.is_none(), "the reader is not pointed at what is gone");
    assert!(store.read(&c, &first.id, None, None).is_err());
    assert_eq!(store.read(&c, &second.id, None, None).unwrap().text, "B");
    assert!(store.tool_read(&c, true, None, None, None).unwrap()["earlier"].as_array().unwrap().is_empty());
    // A session of the same id on another provider, or in another workspace, is someone else's.
    assert_eq!(store.read(&theirs, &kept.id, None, None).unwrap().text, "another project");

    // Forgetting the workspace takes the rest of its archives, and only its own.
    assert_eq!(store.delete_workspace(root.path()).unwrap(), 1);
    assert!(store.bound(&c).unwrap().is_none());
    assert_eq!(files(&data), 1);
    assert_eq!(store.delete_workspace(root.path()).unwrap(), 0);
    assert!(store.read(&theirs, &kept.id, None, None).is_ok());
}

#[test]
fn what_is_kept_is_bounded_oldest_first_and_abandoned_archives_go_before_that() {
    let data = TempDir::new().unwrap(); let root = TempDir::new().unwrap();
    let mut store = Store::open(data.path()).unwrap();
    assert_eq!(store.limits, Limits::default());

    // A hand-off that never completed leaves its archive unbound: a session keeps only its newest few.
    store.limits = Limits { unbound_per_session: 2, ..Limits::default() };
    let a = identity(&root, "a"); let b = identity(&root, "b");
    let handed = store.save(&a, "handed over").unwrap();
    store.attach(&a, &b, &handed.id).unwrap();
    let abandoned: Vec<_> = (0..4).map(|round| store.save(&a, &format!("attempt {round}")).unwrap()).collect();
    assert!(store.read(&a, &abandoned[0].id, None, None).is_err() && store.read(&a, &abandoned[1].id, None, None).is_err());
    assert!(store.read(&a, &abandoned[2].id, None, None).is_ok() && store.read(&a, &abandoned[3].id, None, None).is_ok());
    assert!(store.read(&b, &handed.id, None, None).is_ok(), "one a session was handed is not an abandoned one");
    assert_eq!(files(&data), 3);
    store.delete_workspace(root.path()).unwrap();

    // By count: the oldest archives go, with the bindings to them.
    store.limits = Limits { archives: 3, ..Limits::default() };
    let sessions: Vec<Identity> = (0..5).map(|n| identity(&root, &format!("s{n}"))).collect();
    let mut saved = Vec::new();
    for pair in sessions.windows(2) {
        let archive = store.save(&pair[0], "history").unwrap();
        store.attach(&pair[0], &pair[1], &archive.id).unwrap();
        saved.push(archive);
    }
    assert_eq!(saved.len(), 4);
    assert_eq!(files(&data), 3);
    assert!(store.bound(&sessions[1]).unwrap().is_none(), "the oldest went, and its binding with it");
    assert_eq!(store.bound(&sessions[4]).unwrap().unwrap().id, saved[3].id);
    store.delete_workspace(root.path()).unwrap();

    // By size: the one just written always stays, whatever it pushes out.
    store.limits = Limits { total_bytes: 25, ..Limits::default() };
    let first = store.save(&sessions[0], &"x".repeat(10)).unwrap();
    let second = store.save(&sessions[1], &"y".repeat(10)).unwrap();
    assert_eq!(files(&data), 2);
    let third = store.save(&sessions[2], &"z".repeat(10)).unwrap();
    assert!(store.read(&sessions[0], &first.id, None, None).is_err());
    assert!(store.read(&sessions[1], &second.id, None, None).is_ok() && store.read(&sessions[2], &third.id, None, None).is_ok());
    let large = store.save(&sessions[3], &"w".repeat(40)).unwrap();
    assert_eq!(files(&data), 1);
    assert!(store.read(&sessions[3], &large.id, None, None).is_ok());
}

#[cfg(unix)]
#[test]
fn symlink_archive_is_rejected() {
    let data=TempDir::new().unwrap(); let root=TempDir::new().unwrap(); let a=identity(&root,"a");
    let store=Store::open(data.path()).unwrap(); let archive=store.save(&a,"secret").unwrap();
    std::fs::remove_file(&archive.path).unwrap();
    std::os::unix::fs::symlink(root.path().join("outside"),&archive.path).unwrap();
    assert!(store.read(&a,&archive.id,None,None).is_err());
}

#[cfg(windows)]
#[test]
fn junction_storage_escape_is_rejected() {
    let data = TempDir::new().unwrap(); let outside = TempDir::new().unwrap();
    junction::create(outside.path(), data.path().join("continuity")).unwrap();
    assert!(Store::open(data.path()).is_err());
    junction::delete(data.path().join("continuity")).unwrap();
}
