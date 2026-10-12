#![cfg(unix)]

use std::{
    fs,
    os::unix::fs::{symlink, PermissionsExt},
    path::{Path, PathBuf},
};

use super::{nofollow, ReadError, MAX_CONTENT_BYTES, MAX_LIST_WINDOW};

fn setup() -> (tempfile::TempDir, tempfile::TempDir) {
    (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap())
}

/// Replaces `workspace/dir` with a symlink to `outside`, as an attacker could after policy
/// has already approved the canonical path `workspace/dir/...`.
fn swap_dir_for_symlink(workspace: &Path, outside: &Path) -> PathBuf {
    let dir = workspace.join("dir");
    fs::rename(&dir, workspace.join("dir.moved")).unwrap();
    symlink(outside, &dir).unwrap();
    dir
}

fn page(path: &Path, offset: usize, limit: usize) -> serde_json::Value {
    match nofollow::read(path, offset, limit, &Default::default()) {
        Ok(value) => value,
        Err(_) => panic!("read failed"),
    }
}

#[test]
fn parent_swapped_for_symlink_after_authorization_is_refused() {
    let (workspace, outside) = setup();
    fs::create_dir(workspace.path().join("dir")).unwrap();
    fs::write(workspace.path().join("dir/file"), "inside").unwrap();
    fs::write(outside.path().join("file"), "secret").unwrap();
    let approved = fs::canonicalize(workspace.path()).unwrap().join("dir/file");
    let approved_new = fs::canonicalize(workspace.path()).unwrap().join("dir/new");

    let dir = swap_dir_for_symlink(workspace.path(), outside.path());
    assert!(dir.is_symlink());

    assert!(nofollow::read(&approved, 0, 10, &Default::default()).is_err());
    assert!(nofollow::write(&approved, b"pwned", &Default::default()).is_err());
    assert!(nofollow::write(&approved_new, b"pwned", &Default::default()).is_err());
    assert!(nofollow::edit(&approved, "secret", "pwned", &Default::default()).is_err());

    assert_eq!(
        fs::read_to_string(outside.path().join("file")).unwrap(),
        "secret"
    );
    let leftovers: Vec<_> = fs::read_dir(outside.path()).unwrap().collect();
    assert_eq!(
        leftovers.len(),
        1,
        "nothing may be created outside the root"
    );
}

#[test]
fn directory_listing_through_swapped_parent_is_refused() {
    let (workspace, outside) = setup();
    fs::create_dir_all(workspace.path().join("dir/sub")).unwrap();
    fs::write(outside.path().join("secret-name"), "x").unwrap();
    let approved = fs::canonicalize(workspace.path()).unwrap().join("dir/sub");
    swap_dir_for_symlink(workspace.path(), outside.path());
    assert!(nofollow::read(&approved, 0, 10, &Default::default()).is_err());
    let approved_root = fs::canonicalize(workspace.path()).unwrap().join("dir");
    assert!(nofollow::read(&approved_root, 0, 10, &Default::default()).is_err());
}

#[test]
fn final_component_swapped_for_symlink_is_refused() {
    let (workspace, outside) = setup();
    let approved = fs::canonicalize(workspace.path()).unwrap().join("file");
    fs::write(&approved, "inside").unwrap();
    fs::write(outside.path().join("file"), "secret").unwrap();
    fs::remove_file(&approved).unwrap();
    symlink(outside.path().join("file"), &approved).unwrap();

    assert!(nofollow::read(&approved, 0, 10, &Default::default()).is_err());
    assert!(nofollow::write(&approved, b"pwned", &Default::default()).is_err());
    assert!(nofollow::edit(&approved, "secret", "pwned", &Default::default()).is_err());
    assert_eq!(
        fs::read_to_string(outside.path().join("file")).unwrap(),
        "secret"
    );
    assert!(approved.is_symlink());
}

#[test]
fn write_and_edit_replace_atomically_keep_mode_and_leave_no_temp_files() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    let file = root.join("file");

    nofollow::write(&file, b"before", &Default::default()).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();
    nofollow::write(&file, b"before again", &Default::default()).unwrap();
    assert_eq!(fs::read_to_string(&file).unwrap(), "before again");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );

    assert!(nofollow::edit(&file, "again", "after", &Default::default()).unwrap());
    assert_eq!(fs::read_to_string(&file).unwrap(), "before after");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );

    assert!(!nofollow::edit(&file, "missing", "x", &Default::default()).unwrap());
    fs::write(&file, "aa").unwrap();
    assert!(!nofollow::edit(&file, "a", "b", &Default::default()).unwrap());
    assert_eq!(fs::read_to_string(&file).unwrap(), "aa");

    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn write_refuses_directories_and_relative_paths() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    fs::create_dir(root.join("dir")).unwrap();
    assert!(nofollow::write(&root.join("dir"), b"x", &Default::default()).is_err());
    assert!(nofollow::write(Path::new("relative"), b"x", &Default::default()).is_err());
    assert!(nofollow::write(&root.join("a/../b"), b"x", &Default::default()).is_err());
    assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
}

#[test]
fn read_returns_lines_pages_and_directory_entries() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    fs::write(root.join("file"), "a\r\nb\n\nc").unwrap();
    let first = page(&root.join("file"), 0, 2);
    assert_eq!(first["content"], "a\nb\n");
    assert_eq!(first["next_offset"], 2);
    assert_eq!(first["more"], true);
    assert!(first.get("truncated").is_none());
    let rest = page(&root.join("file"), 2, 10);
    assert_eq!(rest["content"], "\nc\n");
    assert_eq!(rest["more"], false);
    let exact = page(&root.join("file"), 0, 4);
    assert_eq!(exact["more"], false);

    fs::write(root.join("bad"), [0xff, b'\n']).unwrap();
    assert!(nofollow::read(&root.join("bad"), 0, 10, &Default::default()).is_err());
}

#[test]
fn directory_listing_is_sorted_paged_and_bounded() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    for n in (0..50).rev() {
        fs::write(root.join(format!("f{n:02}")), "").unwrap();
    }
    let first = page(&root, 0, 5);
    assert_eq!(
        first["entries"],
        serde_json::json!(["f00", "f01", "f02", "f03", "f04"])
    );
    assert_eq!(first["more"], true);
    let last = page(&root, 48, 5);
    assert_eq!(last["entries"], serde_json::json!(["f48", "f49"]));
    assert_eq!(last["more"], false);
    assert_eq!(page(&root, 45, 5)["more"], false);
    assert!(matches!(
        nofollow::read(&root, MAX_LIST_WINDOW, 1, &Default::default()),
        Err(ReadError::WindowTooLarge)
    ));
    assert!(matches!(
        nofollow::read(&root, usize::MAX, usize::MAX, &Default::default()),
        Err(ReadError::WindowTooLarge)
    ));
}

#[test]
fn huge_single_line_stays_within_the_page_budget() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    let file = root.join("huge");
    let mut data = vec![b'a'; 8 * 1024 * 1024];
    data.extend_from_slice(b"\nafter\n");
    fs::write(&file, &data).unwrap();

    let first = page(&file, 0, 200);
    let content = first["content"].as_str().unwrap();
    assert!(content.len() <= MAX_CONTENT_BYTES);
    assert!(content.contains("[line truncated]"));
    assert_eq!(first["truncated"], true);
    // The oversized line is consumed, so the next page makes progress.
    assert_eq!(first["next_offset"], 1);
    assert_eq!(first["more"], true);
    let second = page(&file, 1, 200);
    assert_eq!(second["content"], "after\n");
    assert_eq!(second["more"], false);

    // Without a trailing newline the line is cut the same way.
    fs::write(&file, vec![b'b'; 1024 * 1024]).unwrap();
    let only = page(&file, 0, 200);
    assert!(only["content"].as_str().unwrap().len() <= MAX_CONTENT_BYTES);
    assert_eq!(only["more"], false);
}

#[test]
fn oversized_line_after_small_ones_starts_the_next_page() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    let file = root.join("mixed");
    let mut data = b"small\n".to_vec();
    data.extend(vec![b'x'; 4 * MAX_CONTENT_BYTES]);
    data.extend_from_slice(b"\ntail\n");
    fs::write(&file, &data).unwrap();

    let first = page(&file, 0, 200);
    assert_eq!(first["content"], "small\n");
    assert_eq!(first["next_offset"], 1);
    assert_eq!(first["more"], true);
    let second = page(&file, 1, 200);
    assert_eq!(second["truncated"], true);
    assert_eq!(second["next_offset"], 2);
    assert_eq!(second["more"], true);
    assert_eq!(page(&file, 2, 200)["content"], "tail\n");
}

#[test]
fn cut_inside_a_multibyte_character_stays_valid_utf8() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    let file = root.join("wide");
    fs::write(&file, "한".repeat(MAX_CONTENT_BYTES)).unwrap();
    let first = page(&file, 0, 10);
    assert_eq!(first["truncated"], true);
    assert!(first["content"].as_str().unwrap().starts_with('한'));
}

#[cfg(target_os = "linux")]
#[test]
fn reading_a_fifo_does_not_block_and_fails() {
    let workspace = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(workspace.path()).unwrap();
    let fifo = root.join("fifo");
    rustix::fs::mknodat(
        rustix::fs::CWD,
        &fifo,
        rustix::fs::FileType::Fifo,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
        0,
    )
    .unwrap();
    assert!(nofollow::read(&fifo, 0, 10, &Default::default()).is_err());
}

#[test]
fn opened_descriptor_identity_refuses_hardlinks_renames_and_atomic_replacements() {
    use crate::credential_paths::ProtectedPaths;
    let root = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(root.path()).unwrap();
    let source = root.join("source.txt");
    fs::write(&source, "test-identity-secret").unwrap();
    let protected = ProtectedPaths::build(
        &serde_json::json!({"source":{"token_file":source}}),
        None,
        None,
    );
    let alias = root.join("ordinary.txt");
    fs::hard_link(&source, &alias).unwrap();
    assert!(matches!(
        nofollow::read(&alias, 0, 10, &protected),
        Err(ReadError::Protected)
    ));
    assert!(nofollow::write(&alias, b"replacement", &protected).is_err());
    assert!(nofollow::edit(&alias, "test", "changed", &protected).is_err());
    fs::remove_file(&alias).unwrap();
    let renamed = root.join("renamed.txt");
    fs::rename(&source, &renamed).unwrap();
    assert!(matches!(
        nofollow::read(&renamed, 0, 10, &protected),
        Err(ReadError::Protected)
    ));
    fs::write(&source, "test-new-secret").unwrap();
    fs::hard_link(&source, &alias).unwrap();
    assert!(matches!(
        nofollow::read(&alias, 0, 10, &protected),
        Err(ReadError::Protected)
    ));
    assert_eq!(fs::read_to_string(&source).unwrap(), "test-new-secret");
    assert_eq!(
        fs::read_to_string(&renamed).unwrap(),
        "test-identity-secret"
    );
}

#[test]
fn descriptor_guard_catches_a_hardlink_installed_after_path_authorization() {
    use crate::credential_paths::ProtectedPaths;
    let root = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(root.path()).unwrap();
    let source = root.join("source.txt");
    let approved = root.join("ordinary.txt");
    fs::write(&source, "test-descriptor-canary").unwrap();
    fs::write(&approved, "public").unwrap();
    let protected = ProtectedPaths::build(
        &serde_json::json!({"source":{"token_file":source}}),
        None,
        None,
    );
    assert!(!protected.is_protected(&approved));
    fs::remove_file(&approved).unwrap();
    fs::hard_link(&source, &approved).unwrap();
    assert!(matches!(
        nofollow::read(&approved, 0, 10, &protected),
        Err(ReadError::Protected)
    ));
    assert!(nofollow::write(&approved, b"x", &protected).is_err());
    assert!(nofollow::edit(&approved, "test", "x", &protected).is_err());
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        "test-descriptor-canary"
    );
}
