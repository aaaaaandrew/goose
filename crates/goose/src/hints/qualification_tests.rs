use super::*;
use rmcp::model::{object, CallToolRequestParams};
use std::fs;
use tempfile::TempDir;

fn fixture() -> TempDir {
    let root = TempDir::new().unwrap();
    for child in ["a", "b", "c"] {
        fs::create_dir_all(root.path().join("module").join(child)).unwrap();
    }
    fs::write(root.path().join("module/.goosehints"), "parent rule").unwrap();
    root
}

fn access(path: &str) -> Message {
    Message::assistant().with_tool_request(
        path,
        Ok(CallToolRequestParams::new("text_editor")
            .with_arguments(object(serde_json::json!({"path": path})))),
    )
}

#[test]
fn unchanged_parent_is_announced_once_across_children_and_resume() {
    let root = fixture();
    let mut messages = Vec::new();
    let mut announced = Vec::new();
    for path in ["module/a/file", "module/b/file", "module/c/file"] {
        messages.push(access(path));
        let carriers = pending_hint_carriers(&messages, root.path());
        announced.push(carriers.len());
        messages.extend(carriers);
    }
    assert_eq!(announced, vec![1, 0, 0]);
    let serialized = serde_json::to_string(&messages).unwrap();
    let restored: Vec<Message> = serde_json::from_str(&serialized).unwrap();
    assert!(pending_hint_carriers(&restored, root.path()).is_empty());
    assert_eq!(
        fs::read_to_string(root.path().join("module/.goosehints")).unwrap(),
        "parent rule"
    );
}

#[test]
fn compacted_invisible_carriers_restore_sources_once() {
    let root = fixture();
    let mut messages = vec![access("module/a/file"), access("module/b/file")];
    messages.extend(pending_hint_carriers(&messages, root.path()));
    let mut compacted: Vec<Message> = messages
        .into_iter()
        .map(|message| {
            let metadata = message.metadata.clone().with_agent_invisible();
            message.with_metadata(metadata)
        })
        .collect();
    let restored = pending_hint_carriers(&compacted, root.path());
    assert_eq!(restored.len(), 1);
    compacted.extend(restored);
    assert!(pending_hint_carriers(&compacted, root.path()).is_empty());
}

#[test]
fn changed_parent_is_announced_without_repeating_unchanged_child() {
    let root = fixture();
    fs::write(root.path().join("module/a/.goosehints"), "child rule").unwrap();
    let mut messages = vec![access("module/a/file")];
    messages.extend(pending_hint_carriers(&messages, root.path()));
    fs::write(
        root.path().join("module/.goosehints"),
        "updated parent rule",
    )
    .unwrap();
    let revised = pending_hint_carriers(&messages, root.path());
    assert_eq!(revised.len(), 1);
    let sources = revised[0]
        .metadata
        .operation_note(HINT_CARRIER_OPERATION, HINT_CARRIER_SOURCES_NOTE)
        .unwrap();
    assert_eq!(sources.as_array().unwrap().len(), 1);
    assert!(revised[0].as_concat_text().contains("updated parent rule"));
    assert!(!revised[0].as_concat_text().contains("child rule"));
    messages.extend(revised);
    assert!(pending_hint_carriers(&messages, root.path()).is_empty());
}

#[test]
fn changed_import_is_announced_once() {
    let root = fixture();
    fs::write(root.path().join("shared.md"), "original imported rule").unwrap();
    fs::write(root.path().join("module/.goosehints"), "@../shared.md").unwrap();
    let mut messages = vec![access("module/a/file")];
    messages.extend(pending_hint_carriers(&messages, root.path()));
    fs::write(root.path().join("shared.md"), "updated imported rule").unwrap();
    let revised = pending_hint_carriers(&messages, root.path());
    assert_eq!(revised.len(), 1);
    assert!(revised[0]
        .as_concat_text()
        .contains("updated imported rule"));
    messages.extend(revised);
    assert!(pending_hint_carriers(&messages, root.path()).is_empty());
}

#[test]
fn identical_content_from_distinct_sources_is_not_discarded() {
    let root = fixture();
    for child in ["a", "b"] {
        fs::write(
            root.path().join(format!("module/{child}/.goosehints")),
            "shared wording",
        )
        .unwrap();
    }
    let mut messages = vec![access("module/a/file")];
    messages.extend(pending_hint_carriers(&messages, root.path()));
    messages.push(access("module/b/file"));
    let discovered = pending_hint_carriers(&messages, root.path());
    assert_eq!(discovered.len(), 1);
    let sources = discovered[0]
        .metadata
        .operation_note(HINT_CARRIER_OPERATION, HINT_CARRIER_SOURCES_NOTE)
        .unwrap();
    assert_eq!(sources.as_array().unwrap().len(), 1);
}

#[test]
fn carriers_recover_changed_sources_after_tool_history_is_removed() {
    for visible in [true, false] {
        for updated in [true, false] {
            let root = fixture();
            let mut messages = pending_hint_carriers(&[access("module/a/file")], root.path());
            assert_eq!(messages.len(), 1);
            if !visible {
                messages = messages
                    .into_iter()
                    .map(|message| {
                        let metadata = message.metadata.clone().with_agent_invisible();
                        message.with_metadata(metadata)
                    })
                    .collect();
            }
            let rule = if updated {
                "updated parent after tool summarization"
            } else {
                "parent rule"
            };
            fs::write(root.path().join("module/.goosehints"), rule).unwrap();
            let restored = pending_hint_carriers(&messages, root.path());
            assert_eq!(
                restored.len(),
                usize::from(updated || !visible),
                "carrier visibility: {visible}, source updated: {updated}"
            );
            if let Some(carrier) = restored.first() {
                assert!(carrier.as_concat_text().contains(rule));
            }
            messages.extend(restored);
            assert!(pending_hint_carriers(&messages, root.path()).is_empty());
        }
    }
}

#[cfg(unix)]
#[test]
fn symlinked_sources_preserve_directory_relative_import_scopes() {
    use std::os::unix::fs::symlink;

    let root = fixture();
    let shared = root.path().join("shared-hints.md");
    fs::write(&shared, "@policy.md").unwrap();
    for (child, rule) in [("a", "rule for a"), ("b", "rule for b")] {
        fs::write(root.path().join(format!("module/{child}/policy.md")), rule).unwrap();
        symlink(
            &shared,
            root.path().join(format!("module/{child}/.goosehints")),
        )
        .unwrap();
    }
    let mut messages = vec![access("module/a/file")];
    messages.extend(pending_hint_carriers(&messages, root.path()));
    messages.push(access("module/b/file"));
    let discovered = pending_hint_carriers(&messages, root.path());
    assert_eq!(discovered.len(), 1);
    assert!(discovered[0].as_concat_text().contains("rule for b"));
    assert!(!discovered[0].as_concat_text().contains("rule for a"));
    assert!(!discovered[0].as_concat_text().contains("parent rule"));
    messages.extend(discovered);
    assert!(pending_hint_carriers(&messages, root.path()).is_empty());
}

#[test]
#[serial_test::serial]
fn already_global_source_is_not_reannounced_by_descendant_discovery() {
    let root = TempDir::new().unwrap();
    let agents = root.path().join(".agents");
    fs::create_dir_all(agents.join("skills/example")).unwrap();
    fs::write(agents.join(AGENTS_MD_FILENAME), "global rule").unwrap();
    let previous = std::env::var_os("GOOSE_PATH_ROOT");
    std::env::set_var("GOOSE_PATH_ROOT", root.path());
    let system = load_hint_files(
        root.path(),
        &[AGENTS_MD_FILENAME.to_string()],
        &Gitignore::empty(),
    );
    let carriers = pending_hint_carriers(&[access(".agents/skills/example/SKILL.md")], root.path());
    match previous {
        Some(value) => std::env::set_var("GOOSE_PATH_ROOT", value),
        None => std::env::remove_var("GOOSE_PATH_ROOT"),
    }
    assert_eq!(system.matches("global rule").count(), 1);
    assert!(carriers.is_empty());
}

#[test]
#[serial_test::serial]
fn overlapping_global_and_project_scopes_load_source_once() {
    let root = TempDir::new().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    let agents = root.path().join(".agents");
    let project = agents.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(agents.join(AGENTS_MD_FILENAME), "overlapping global rule").unwrap();
    let previous = std::env::var_os("GOOSE_PATH_ROOT");
    std::env::set_var("GOOSE_PATH_ROOT", root.path());
    let system = load_hint_files(
        &project,
        &[AGENTS_MD_FILENAME.to_string()],
        &Gitignore::empty(),
    );
    match previous {
        Some(value) => std::env::set_var("GOOSE_PATH_ROOT", value),
        None => std::env::remove_var("GOOSE_PATH_ROOT"),
    }
    assert_eq!(system.matches("overlapping global rule").count(), 1);
}

#[test]
#[serial_test::serial]
fn overlapping_source_keeps_imports_allowed_by_project_boundary() {
    let root = TempDir::new().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    let agents = root.path().join(".agents");
    let project = agents.join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        agents.join(AGENTS_MD_FILENAME),
        "global rule\n@../policy.md",
    )
    .unwrap();
    fs::write(root.path().join("policy.md"), "project imported rule").unwrap();
    let previous = std::env::var_os("GOOSE_PATH_ROOT");
    std::env::set_var("GOOSE_PATH_ROOT", root.path());
    let system = load_hint_files(
        &project,
        &[AGENTS_MD_FILENAME.to_string()],
        &Gitignore::empty(),
    );
    match previous {
        Some(value) => std::env::set_var("GOOSE_PATH_ROOT", value),
        None => std::env::remove_var("GOOSE_PATH_ROOT"),
    }
    assert!(system.contains("project imported rule"));
}

#[test]
#[serial_test::serial]
fn global_source_discovery_keeps_newly_allowed_project_imports() {
    let root = TempDir::new().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    let agents = root.path().join(".agents");
    fs::create_dir_all(agents.join("skills/example")).unwrap();
    fs::write(
        agents.join(AGENTS_MD_FILENAME),
        "global rule\n@../policy.md",
    )
    .unwrap();
    fs::write(root.path().join("policy.md"), "project imported rule").unwrap();
    let previous = std::env::var_os("GOOSE_PATH_ROOT");
    std::env::set_var("GOOSE_PATH_ROOT", root.path());
    let mut messages = vec![access(".agents/skills/example/SKILL.md")];
    let carriers = pending_hint_carriers(&messages, root.path());
    messages.extend(carriers.clone());
    let repeated = pending_hint_carriers(&messages, root.path());
    match previous {
        Some(value) => std::env::set_var("GOOSE_PATH_ROOT", value),
        None => std::env::remove_var("GOOSE_PATH_ROOT"),
    }
    assert_eq!(carriers.len(), 1);
    assert!(carriers[0]
        .as_concat_text()
        .contains("project imported rule"));
    assert!(repeated.is_empty());
}
