//! Library, project-skill, install-safety and agent-preset behavior on real directories.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::{collections::BTreeMap, ffi::OsString, fs, path::Path};

use openside_skills::{
    agent_skills_dir, bundled, parse, project_dir, validate_bundled, Error, InstallOptions,
    NewResource, NewSkill, SkillUpdate, Source, Store, BROWSER_SKILL,
};

fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
    let dir = root.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    )
    .unwrap();
}

fn new(name: &str) -> NewSkill {
    NewSkill {
        name: name.into(),
        description: format!("Does {name} things"),
        body: format!("# {name}\n\nDo the thing.\n"),
        license: None,
        resources: Vec::new(),
    }
}

#[test]
fn create_edit_toggle_delete_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("skills"));
    assert!(store.scan(None).entries.is_empty());

    let created = store.create(new("alpha")).unwrap();
    assert!(created.enabled && created.source == Source::User);
    assert!(matches!(store.create(new("alpha")), Err(Error::Exists(_))));
    assert!(store.create(new("Not Valid")).is_err());

    let edited = store
        .update(
            "alpha",
            SkillUpdate {
                description: Some("Rewritten: with \"quotes\"".into()),
                body: Some("new body\n".into()),
                enabled: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        edited.skill.frontmatter.description,
        "Rewritten: with \"quotes\""
    );
    assert_eq!(edited.skill.body, "new body\n");

    // Body-only edits keep the original frontmatter bytes (comments, extra keys).
    let path = edited.skill.dir.join("SKILL.md");
    fs::write(
        &path,
        "---\n# kept comment\nname: alpha\ndescription: Hand written\nx-extra: 1\n---\nold\n",
    )
    .unwrap();
    store
        .update(
            "alpha",
            SkillUpdate {
                body: Some("fresh\n".into()),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let text = fs::read_to_string(&path).unwrap();
    assert!(
        text.contains("# kept comment") && text.contains("x-extra: 1") && text.ends_with("fresh\n")
    );

    let off = store
        .update(
            "alpha",
            SkillUpdate {
                enabled: Some(false),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    assert!(!off.enabled);
    assert!(!store.get("alpha", None).unwrap().enabled);
    store.set_enabled("alpha", true).unwrap();
    assert!(store.get("alpha", None).unwrap().enabled);

    store.set_enabled("alpha", false).unwrap();
    store.remove("alpha").unwrap();
    assert!(matches!(store.get("alpha", None), Err(Error::NotFound(_))));
    assert!(matches!(store.remove("alpha"), Err(Error::NotFound(_))));
    // Recreating after delete starts enabled again.
    assert!(store.create(new("alpha")).unwrap().enabled);
}

#[test]
fn invalid_skills_are_reported_without_hiding_valid_ones() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_skill(root, "good", "A good skill", "body");
    fs::create_dir_all(root.join("broken")).unwrap();
    fs::write(
        root.join("broken/SKILL.md"),
        "---\nname: broken\ndescription: [unterminated\n---\n",
    )
    .unwrap();
    write_skill(root, "mismatch", "x", "body");
    fs::write(
        root.join("mismatch/SKILL.md"),
        "---\nname: other\ndescription: x\n---\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("empty-dir")).unwrap();
    fs::write(root.join("notes.txt"), "not a skill").unwrap();
    fs::create_dir_all(root.join(".hidden")).unwrap();

    let scan = Store::new(root).scan(None);
    let names: Vec<_> = scan.entries.iter().map(|e| e.skill.name()).collect();
    assert_eq!(names, ["good"]);
    let mut problems: Vec<_> = scan.problems.iter().map(|p| p.name.as_str()).collect();
    problems.sort_unstable();
    assert_eq!(problems, ["broken", "mismatch"]);
}

#[test]
fn project_skills_are_read_only_and_shadowed_by_user_skills() {
    let dir = tempfile::tempdir().unwrap();
    let user = dir.path().join("user");
    let workspace = dir.path().join("repo");
    let project = project_dir(&workspace);
    write_skill(&user, "shared", "user version", "u");
    write_skill(&project, "shared", "project version", "p");
    write_skill(&project, "local-only", "project only", "p");
    let store = Store::new(&user);
    let scan = store.scan(Some(&project));
    assert_eq!(scan.entries.len(), 2);
    assert_eq!(
        store
            .get("shared", Some(&project))
            .unwrap()
            .skill
            .frontmatter
            .description,
        "user version"
    );
    let local = store.get("local-only", Some(&project)).unwrap();
    assert_eq!(local.source, Source::Project);
    assert!(matches!(
        store.update(
            "local-only",
            SkillUpdate {
                body: Some("x".into()),
                ..Default::default()
            },
            Some(&project)
        ),
        Err(Error::ReadOnly(_))
    ));
    // Switching a project skill off only touches the user's state file.
    let off = store
        .update(
            "local-only",
            SkillUpdate {
                enabled: Some(false),
                ..Default::default()
            },
            Some(&project),
        )
        .unwrap();
    assert!(!off.enabled && off.source == Source::Project);
    assert!(user.join(".state.json").is_file());
    assert!(fs::read_to_string(project.join("local-only/SKILL.md"))
        .unwrap()
        .contains("project only"));
}

#[cfg(unix)]
#[test]
fn project_skill_symlinks_cannot_escape_the_project_directory() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    write_skill(&outside, "secret-skill", "outside", "body");
    let project = dir.path().join("repo/.openside/skills");
    fs::create_dir_all(&project).unwrap();
    symlink(outside.join("secret-skill"), project.join("secret-skill")).unwrap();
    write_skill(&project, "inside", "inside", "body");
    let scan = Store::new(dir.path().join("user")).scan(Some(&project));
    assert_eq!(scan.entries.len(), 1);
    assert_eq!(scan.entries[0].skill.name(), "inside");
    assert_eq!(scan.problems.len(), 1);
    assert!(
        scan.problems[0].error.contains("outside"),
        "{:?}",
        scan.problems
    );
}

#[cfg(unix)]
#[test]
fn user_library_follows_symlinked_skill_directories() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = dir.path().join("elsewhere");
    write_skill(&elsewhere, "linked", "Symlinked skill folder", "body");
    let user = dir.path().join("user");
    fs::create_dir_all(&user).unwrap();
    symlink(elsewhere.join("linked"), user.join("linked")).unwrap();
    let store = Store::new(&user);
    assert_eq!(store.get("linked", None).unwrap().skill.body, "body");
    // Deleting unlinks the symlink and leaves the target alone.
    store.remove("linked").unwrap();
    assert!(elsewhere.join("linked/SKILL.md").is_file());
}

#[test]
fn resources_are_listed_and_read_inside_the_skill_only() {
    let dir = tempfile::tempdir().unwrap();
    write_skill(dir.path(), "res", "has resources", "body");
    fs::create_dir_all(dir.path().join("res/references")).unwrap();
    fs::write(dir.path().join("res/references/guide.md"), "guide text").unwrap();
    fs::write(dir.path().join("res/.secret"), "hidden").unwrap();
    fs::write(dir.path().join("outside.txt"), "outside").unwrap();
    let skill = Store::new(dir.path()).get("res", None).unwrap().skill;
    let listed: Vec<_> = skill.resources().into_iter().map(|r| r.path).collect();
    assert_eq!(listed, ["references/guide.md"]);
    assert_eq!(
        skill.read_resource("references/guide.md").unwrap(),
        "guide text"
    );
    for bad in [
        "../outside.txt",
        "/etc/passwd",
        ".secret",
        "references/../../outside.txt",
        "",
    ] {
        assert!(skill.read_resource(bad).is_err(), "{bad}");
    }
    assert!(matches!(
        skill.read_resource("references/none.md"),
        Err(Error::NotFound(_))
    ));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            dir.path().join("outside.txt"),
            dir.path().join("res/link.txt"),
        )
        .unwrap();
        assert!(matches!(
            skill.read_resource("link.txt"),
            Err(Error::Unsafe(_))
        ));
        assert!(!skill.resources().iter().any(|r| r.path == "link.txt"));
    }
    fs::write(dir.path().join("res/big.md"), vec![b'x'; 40 * 1024]).unwrap();
    assert!(matches!(
        skill.read_resource("big.md"),
        Err(Error::TooLarge { .. })
    ));
}

#[test]
fn import_copies_validates_and_refuses_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("src");
    write_skill(&source, "copy-me", "To be copied", "body");
    fs::create_dir_all(source.join("copy-me/refs")).unwrap();
    fs::write(source.join("copy-me/refs/a.md"), "a").unwrap();
    fs::create_dir_all(source.join("copy-me/.git")).unwrap();
    fs::write(source.join("copy-me/.git/config"), "x").unwrap();
    let store = Store::new(dir.path().join("lib"));
    let installed = store
        .import(&source.join("copy-me"), InstallOptions::default())
        .unwrap();
    assert_eq!((installed.name.as_str(), installed.files), ("copy-me", 2));
    assert!(installed.path.join("refs/a.md").is_file());
    assert!(!installed.path.join(".git").exists());
    assert!(matches!(
        store.import(&source.join("copy-me"), InstallOptions::default()),
        Err(Error::Exists(_))
    ));
    fs::write(source.join("copy-me/refs/a.md"), "changed").unwrap();
    store
        .import(
            &source.join("copy-me"),
            InstallOptions {
                force: true,
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        fs::read_to_string(installed.path.join("refs/a.md")).unwrap(),
        "changed"
    );
    // No staging directories remain.
    let leftovers: Vec<_> = fs::read_dir(store.root())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[cfg(unix)]
#[test]
fn install_refuses_escaping_symlinks_executables_and_bad_skills() {
    use std::os::unix::fs::{symlink, PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path().join("lib"));

    let escape = dir.path().join("escape");
    write_skill(&escape, "escape", "has a bad link", "body");
    fs::write(dir.path().join("host-secret"), "secret").unwrap();
    symlink(dir.path().join("host-secret"), escape.join("escape/leak")).unwrap();
    assert!(matches!(
        store.import(&escape.join("escape"), InstallOptions::default()),
        Err(Error::Unsafe(_))
    ));
    assert!(!store.root().join("escape").exists());

    let script = dir.path().join("script");
    write_skill(&script, "script", "has a script", "body");
    fs::create_dir_all(script.join("script/bin")).unwrap();
    let run = script.join("script/bin/run.sh");
    fs::write(&run, "#!/bin/sh\necho hi\n").unwrap();
    fs::set_permissions(&run, fs::Permissions::from_mode(0o755)).unwrap();
    let error = store
        .import(&script.join("script"), InstallOptions::default())
        .unwrap_err();
    assert!(
        matches!(error, Error::Unsafe(_)) && error.to_string().contains("allow-scripts"),
        "{error}"
    );
    assert!(!store.root().join("script").exists());
    let installed = store
        .import(
            &script.join("script"),
            InstallOptions {
                allow_scripts: true,
                ..Default::default()
            },
        )
        .unwrap();
    let mode = fs::metadata(installed.path.join("bin/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);

    let bad = dir.path().join("bad");
    fs::create_dir_all(bad.join("bad")).unwrap();
    fs::write(bad.join("bad/SKILL.md"), "no frontmatter at all").unwrap();
    assert!(store
        .import(&bad.join("bad"), InstallOptions::default())
        .is_err());
    assert!(!store.root().join("bad").exists());
    let none = dir.path().join("none");
    fs::create_dir_all(&none).unwrap();
    assert!(matches!(
        store.import(&none, InstallOptions::default()),
        Err(Error::NotFound(_))
    ));

    // Looping symlink inside the tree is refused, not followed forever.
    let looping = dir.path().join("loop");
    write_skill(&looping, "loop", "loops", "body");
    symlink(looping.join("loop"), looping.join("loop/again")).unwrap();
    assert!(store
        .import(&looping.join("loop"), InstallOptions::default())
        .is_err());
}

#[test]
fn bundled_browser_skill_is_valid_and_installs_into_another_agent_directory() {
    validate_bundled().unwrap();
    let files = bundled(BROWSER_SKILL).unwrap();
    assert!(bundled("unknown").is_none());
    let text = std::str::from_utf8(&files[0].content).unwrap();
    assert_eq!(parse(text).unwrap().frontmatter.name, BROWSER_SKILL);
    let dir = tempfile::tempdir().unwrap();
    let installed =
        openside_skills::install::write_skill(dir.path(), BROWSER_SKILL, files, false).unwrap();
    assert!(installed.path.join("SKILL.md").is_file());
    assert!(matches!(
        openside_skills::install::write_skill(
            dir.path(),
            BROWSER_SKILL,
            bundled(BROWSER_SKILL).unwrap(),
            false
        ),
        Err(Error::Exists(_))
    ));
}

#[test]
fn agent_presets_resolve_from_config_env_then_convention() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |key: &str| {
            vars.iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    };
    let none = BTreeMap::new();
    let home = env(&[("HOME", "/h")]);
    assert_eq!(
        agent_skills_dir("claude", &none, &home).unwrap(),
        Path::new("/h/.claude/skills")
    );
    assert_eq!(
        agent_skills_dir("codex", &none, &home).unwrap(),
        Path::new("/h/.codex/skills")
    );
    assert_eq!(
        agent_skills_dir("omp", &none, &home).unwrap(),
        Path::new("/h").join(".omp").join("agent").join("skills")
    );
    let omp = env(&[("HOME", "/h"), ("PI_CODING_AGENT_DIR", "/pi")]);
    assert_eq!(
        agent_skills_dir("omp", &none, &omp).unwrap(),
        Path::new("/pi/skills")
    );
    let omp = env(&[
        ("HOME", "/h"),
        ("PI_CODING_AGENT_DIR", "/pi"),
        ("OMP_CODING_AGENT_DIR", "/omp"),
    ]);
    assert_eq!(
        agent_skills_dir("omp", &none, &omp).unwrap(),
        Path::new("/omp/skills")
    );
    assert!(
        agent_skills_dir("omp", &none, &env(&[])).is_none(),
        "no HOME, no convention"
    );
    assert!(agent_skills_dir("other", &none, &home).is_none());
    let vars = env(&[
        ("HOME", "/h"),
        ("CLAUDE_CONFIG_DIR", "/c"),
        ("OPENSIDE_SKILLS_OTHER_AGENT_DIR", "/o"),
    ]);
    assert_eq!(
        agent_skills_dir("claude", &none, &vars).unwrap(),
        Path::new("/c/skills")
    );
    assert_eq!(
        agent_skills_dir("other-agent", &none, &vars).unwrap(),
        Path::new("/o")
    );
    let configured = BTreeMap::from([("claude".to_owned(), "/cfg".into())]);
    assert_eq!(
        agent_skills_dir("claude", &configured, &vars).unwrap(),
        Path::new("/cfg")
    );
    for bad in ["", "../x", "A", "a/b"] {
        assert!(agent_skills_dir(bad, &configured, &vars).is_none(), "{bad}");
    }
}

#[test]
fn created_skills_can_carry_text_resources_but_not_escape_or_shadow_skill_md() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path());
    let with = |path: &str| NewSkill {
        resources: vec![NewResource {
            path: path.into(),
            content: "text".into(),
        }],
        ..new("with-files")
    };
    let created = store
        .create(NewSkill {
            resources: vec![
                NewResource {
                    path: "references/guide.md".into(),
                    content: "guide".into(),
                },
                NewResource {
                    path: "notes.txt".into(),
                    content: "notes".into(),
                },
            ],
            ..new("with-files")
        })
        .unwrap();
    assert_eq!(
        created.skill.read_resource("references/guide.md").unwrap(),
        "guide"
    );
    store.remove("with-files").unwrap();
    for bad in ["../x", "/abs", ".hidden", "a/../../b", "SKILL.md", ""] {
        assert!(store.create(with(bad)).is_err(), "{bad}");
        assert!(
            store.get("with-files", None).is_err(),
            "{bad} left a skill behind"
        );
    }
    let leftovers = fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(
        leftovers, 0,
        "failed creates must not leave staging directories"
    );
}

#[cfg(unix)]
#[test]
fn a_symlinked_project_skills_directory_is_ignored_not_trusted() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let outside = dir.path().join("outside");
    write_skill(&outside, "stolen", "lives outside the repo", "x");
    let store = Store::new(dir.path().join("user"));

    // `.openside/skills` itself points away from the repository.
    let repo = dir.path().join("repo-a");
    std::fs::create_dir_all(repo.join(".openside")).unwrap();
    symlink(&outside, repo.join(".openside/skills")).unwrap();
    let scan = store.scan(Some(&project_dir(&repo)));
    assert!(scan.entries.is_empty(), "{scan:?}");
    assert_eq!(scan.problems.len(), 1);
    assert_eq!(scan.problems[0].source, Source::Project);
    assert!(matches!(
        store.get("stolen", Some(&project_dir(&repo))),
        Err(Error::NotFound(_))
    ));

    // `.openside` is the symlink.
    let repo = dir.path().join("repo-b");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(dir.path().join("elsewhere")).unwrap();
    symlink(&outside, dir.path().join("elsewhere/skills")).unwrap();
    symlink(dir.path().join("elsewhere"), repo.join(".openside")).unwrap();
    let scan = store.scan(Some(&project_dir(&repo)));
    assert!(scan.entries.is_empty(), "{scan:?}");
    assert_eq!(scan.problems.len(), 1);
}

#[test]
fn write_skill_rejects_unsafe_or_duplicate_staged_paths() {
    use openside_skills::install::{write_skill as place, StagedFile};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("skills");
    let file = |path: &str, text: &str| StagedFile {
        path: path.into(),
        content: text.as_bytes().to_vec(),
        executable: false,
    };
    let skill_md = || {
        file(
            "SKILL.md",
            "---\nname: staged\ndescription: Staged\n---\nbody",
        )
    };
    for bad in [
        "../escape.txt",
        "/abs/escape.txt",
        "a/../../escape.txt",
        ".hidden",
        "refs/./x.md",
        "refs//x.md",
        "",
    ] {
        let result = place(&root, "staged", vec![skill_md(), file(bad, "x")], false);
        assert!(
            matches!(result, Err(Error::Invalid { .. })),
            "{bad:?}: {result:?}"
        );
    }
    let duplicate = place(
        &root,
        "staged",
        vec![skill_md(), file("a.md", "1"), file("a.md", "2")],
        false,
    );
    assert!(
        matches!(duplicate, Err(Error::Invalid { .. })),
        "{duplicate:?}"
    );
    let twice = place(&root, "staged", vec![skill_md(), skill_md()], false);
    assert!(matches!(twice, Err(Error::Invalid { .. })), "{twice:?}");
    assert!(!root.join("staged").exists());
    assert!(!dir.path().join("escape.txt").exists());
    // Nothing is left behind in staging either.
    assert!(fs::read_dir(&root).map_or(true, |mut entries| entries.next().is_none()));

    place(
        &root,
        "staged",
        vec![skill_md(), file("refs/x.md", "ok")],
        false,
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(root.join("staged/refs/x.md")).unwrap(),
        "ok"
    );
}
