//! File-shaped projection of memory (D5): `MEMORY.md` holds notes about work and context,
//! `USER.md` the notes tagged `profile`, `TAXONOMY.md` the tags with counts. The files are
//! rendered on read and parsed back on save; the store stays the only source of truth.

use std::collections::{BTreeMap, HashMap, HashSet};

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use openside_memory::{MemoryEdit, NewMemory, NoteSnapshot};
use openside_proto::{
    Memory, MemoryChangeList, MemoryFile, MemoryFileInfo, MemoryFileList, MemoryFileSaved,
    MemoryRebuild, MemoryView, UpdateMemoryFileRequest,
};
use serde::Deserialize;
use serde_json::Value;

use super::{error, redacted, API_ACTOR};
use crate::{api::HttpError, engine::Shared};

/// Tag that places a note in `USER.md`.
pub(crate) const PROFILE_TAG: &str = "profile";
const MARKER: &str = "<!-- memory";
const MARKER_END: &str = "-->";
const MAX_FILE: usize = 4 * 1024 * 1024;

fn in_view(view: MemoryView, memory: &Memory) -> bool {
    let profile = memory.tags.iter().any(|tag| tag == PROFILE_TAG);
    match view {
        MemoryView::Memory => !profile,
        MemoryView::User => profile,
        MemoryView::Taxonomy => false,
    }
}

fn stem(view: MemoryView) -> &'static str {
    view.file_name().trim_end_matches(".md")
}

/// Mask Vault values that were unlocked after a note was written.
fn mask(s: &Shared, text: &str) -> Result<String, HttpError> {
    match s
        .registry
        .redact(Value::String(text.to_owned()))
        .map_err(|_| HttpError::internal())?
    {
        Value::String(masked) => Ok(masked),
        _ => Err(HttpError::internal()),
    }
}

/// Lines as the file parser reads them: `\r\n` and stray `\r` at line ends are dropped.
fn file_lines(text: &str) -> impl Iterator<Item = &str> {
    text.lines().map(|line| line.trim_end_matches('\r'))
}

/// A note's text as its file block parses back: normalized line ends, outer whitespace
/// trimmed. An unchanged block equals this, so it is never saved as an update.
fn as_shown(text: &str) -> String {
    file_lines(text)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Whether `line`, minus any leading backslashes, would read as a marker.
fn marker_like(line: &str) -> bool {
    parse_marker(line.trim_start_matches('\\')).is_some()
}

/// Write a note's text into a block. A text line that would read as a marker gets one more
/// leading `\`, which [`parse`] removes, so note text can never split or forge blocks.
fn escape(text: &str) -> String {
    file_lines(text)
        .map(|line| {
            if marker_like(line) {
                format!("\\{line}")
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn marker(memory: &Memory) -> String {
    if memory.tags.is_empty() {
        format!("{MARKER} id={} {MARKER_END}", memory.id)
    } else {
        let tags = serde_json::to_string(&memory.tags).unwrap_or_else(|_| "[]".into());
        format!("{MARKER} id={} tags={tags} {MARKER_END}", memory.id)
    }
}

fn render(s: &Shared, view: MemoryView, snapshot: &NoteSnapshot) -> Result<MemoryFile, HttpError> {
    let mut markdown = format!("# {}\n", stem(view));
    let mut updated_at = None;
    match view {
        MemoryView::Taxonomy => {
            let mut tags = BTreeMap::<&str, u32>::new();
            for note in &snapshot.notes {
                for tag in &note.tags {
                    *tags.entry(tag.as_str()).or_default() += 1;
                }
                updated_at = updated_at.max(Some(note.updated_at));
            }
            markdown.push_str("\n## tags\n\n");
            for (tag, count) in &tags {
                markdown.push_str(&format!("- {}: {count}\n", mask(s, tag)?));
            }
            markdown.push_str(&format!(
                "\n## kinds\n\n- note: {}\n- history: {}\n",
                snapshot.notes.len(),
                snapshot.history
            ));
        }
        _ => {
            for note in snapshot.notes.iter().filter(|note| in_view(view, note)) {
                let masked: Memory = redacted(s, note.clone())?;
                markdown.push_str(&format!(
                    "\n{}\n{}\n",
                    marker(&masked),
                    escape(&as_shown(&masked.content))
                ));
                updated_at = updated_at.max(Some(note.updated_at));
            }
        }
    }
    Ok(MemoryFile {
        name: view.file_name().into(),
        view,
        editable: view != MemoryView::Taxonomy,
        updated_at,
        revision: snapshot.revision,
        markdown,
    })
}

/// One block of an edited file.
#[derive(Debug, PartialEq)]
pub(crate) struct Block {
    pub(crate) id: Option<String>,
    pub(crate) tags: Option<Vec<String>>,
    pub(crate) content: String,
}

/// A block marker's optional id and tags.
type Marker = (Option<String>, Option<Vec<String>>);

fn parse_marker(line: &str) -> Option<Result<Marker, ()>> {
    let inner = line
        .trim_end()
        .strip_prefix(MARKER)?
        .strip_suffix(MARKER_END)?;
    if !(inner.is_empty() || inner.starts_with(char::is_whitespace)) {
        return None;
    }
    let mut rest = inner.trim();
    let (mut id, mut tags) = (None, None);
    while !rest.is_empty() {
        if let Some(value) = rest.strip_prefix("id=") {
            let end = value.find(char::is_whitespace).unwrap_or(value.len());
            if id.is_some() || end == 0 {
                return Some(Err(()));
            }
            id = Some(value[..end].to_owned());
            rest = value[end..].trim_start();
        } else if let Some(value) = rest.strip_prefix("tags=") {
            let mut stream = serde_json::Deserializer::from_str(value).into_iter::<Vec<String>>();
            match stream.next() {
                Some(Ok(parsed)) if tags.is_none() => tags = Some(parsed),
                _ => return Some(Err(())),
            }
            rest = value[stream.byte_offset()..].trim_start();
        } else {
            return Some(Err(()));
        }
    }
    Some(Ok((id, tags)))
}

/// Split a file into blocks. Text before the first marker (minus the `# STEM` heading) is
/// one new note; each marker starts a block that runs to the next marker.
pub(crate) fn parse(view: MemoryView, markdown: &str) -> Result<Vec<Block>, HttpError> {
    let invalid = || HttpError(StatusCode::BAD_REQUEST, "invalid_value");
    let heading = format!("# {}", stem(view));
    let mut blocks = Vec::new();
    let mut current = Block {
        id: None,
        tags: None,
        content: String::new(),
    };
    let mut heading_seen = false;
    let mut preamble = true;
    for line in file_lines(markdown) {
        if let Some(parsed) = parse_marker(line) {
            let (id, tags) = parsed.map_err(|()| invalid())?;
            blocks.push(std::mem::replace(
                &mut current,
                Block {
                    id,
                    tags,
                    content: String::new(),
                },
            ));
            preamble = false;
            continue;
        }
        if preamble && !heading_seen && current.content.trim().is_empty() && line.trim() == heading
        {
            heading_seen = true;
            continue;
        }
        current
            .content
            .push_str(if line.starts_with('\\') && marker_like(line) {
                &line[1..]
            } else {
                line
            });
        current.content.push('\n');
    }
    blocks.push(current);
    for block in &mut blocks {
        block.content = block.content.trim().to_owned();
    }
    blocks.retain(|block| block.id.is_some() || !block.content.is_empty());
    Ok(blocks)
}

/// Turn an edited file into store edits against `snapshot`.
fn diff(
    s: &Shared,
    view: MemoryView,
    snapshot: &NoteSnapshot,
    blocks: Vec<Block>,
) -> Result<Vec<MemoryEdit>, HttpError> {
    let notes: HashMap<&str, &Memory> = snapshot
        .notes
        .iter()
        .filter(|note| in_view(view, note))
        .map(|note| (note.id.as_str(), note))
        .collect();
    let mut seen = HashSet::new();
    let mut edits = Vec::new();
    for block in blocks {
        let Some(id) = block.id else {
            let mut tags = block.tags.unwrap_or_default();
            if view == MemoryView::User && !tags.iter().any(|tag| tag == PROFILE_TAG) {
                tags.push(PROFILE_TAG.into());
            }
            edits.push(MemoryEdit::Create(NewMemory {
                tags,
                ..NewMemory::note(block.content)
            }));
            continue;
        };
        if !seen.insert(id.clone()) {
            return Err(HttpError(StatusCode::BAD_REQUEST, "invalid_value"));
        }
        // An id this view does not show was deleted, moved or never existed: the file is
        // stale, so the whole save is refused rather than guessed.
        let note = notes
            .get(id.as_str())
            .ok_or(HttpError(StatusCode::CONFLICT, "memory_conflict"))?;
        if block.content.is_empty() {
            edits.push(MemoryEdit::Delete { id });
            continue;
        }
        let masked = mask(s, &note.content)?;
        let shown = as_shown(&masked);
        let shown_tags = note
            .tags
            .iter()
            .map(|tag| mask(s, tag))
            .collect::<Result<Vec<_>, _>>()?;
        let tags = block.tags.unwrap_or_else(|| shown_tags.clone());
        if block.content == shown && tags == shown_tags {
            continue;
        }
        // Saving masked text would replace the hidden value with the mask for good.
        if masked != note.content || shown_tags != note.tags {
            return Err(HttpError(StatusCode::BAD_REQUEST, "redacted_memory"));
        }
        edits.push(MemoryEdit::Update {
            id,
            content: block.content,
            tags,
        });
    }
    for id in notes.keys() {
        if !seen.contains(*id) {
            edits.push(MemoryEdit::Delete {
                id: (*id).to_owned(),
            });
        }
    }
    Ok(edits)
}

fn view_of(name: &str) -> Result<MemoryView, HttpError> {
    MemoryView::from_file_name(name).ok_or_else(HttpError::not_found)
}

async fn snapshot(s: &Shared) -> Result<NoteSnapshot, HttpError> {
    s.memory.run(|store| store.notes()).await.map_err(error)
}

pub(crate) async fn list(State(s): State<Shared>) -> Result<Json<MemoryFileList>, HttpError> {
    let snapshot = snapshot(&s).await?;
    let files = MemoryView::ALL
        .into_iter()
        .map(|view| {
            let file = render(&s, view, &snapshot)?;
            let items = match view {
                MemoryView::Taxonomy => snapshot
                    .notes
                    .iter()
                    .flat_map(|note| note.tags.iter())
                    .collect::<HashSet<_>>()
                    .len(),
                _ => snapshot
                    .notes
                    .iter()
                    .filter(|note| in_view(view, note))
                    .count(),
            };
            Ok(MemoryFileInfo {
                name: file.name,
                view,
                editable: file.editable,
                items: u32::try_from(items).unwrap_or(u32::MAX),
                updated_at: file.updated_at,
            })
        })
        .collect::<Result<Vec<_>, HttpError>>()?;
    Ok(Json(MemoryFileList { files }))
}

pub(crate) async fn get(
    State(s): State<Shared>,
    Path(name): Path<String>,
) -> Result<Json<MemoryFile>, HttpError> {
    let view = view_of(&name)?;
    let snapshot = snapshot(&s).await?;
    Ok(Json(render(&s, view, &snapshot)?))
}

pub(crate) async fn put(
    State(s): State<Shared>,
    Path(name): Path<String>,
    Json(request): Json<UpdateMemoryFileRequest>,
) -> Result<Json<MemoryFileSaved>, HttpError> {
    let view = view_of(&name)?;
    if view == MemoryView::Taxonomy {
        return Err(HttpError(StatusCode::BAD_REQUEST, "read_only_view"));
    }
    if request.markdown.len() > MAX_FILE {
        return Err(HttpError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        ));
    }
    let blocks = parse(view, &request.markdown)?;
    let current = snapshot(&s).await?;
    // Ids are checked against the current store, so a note created after the GET is
    // seen as deleted by the file; apply_edits turns that into memory_conflict.
    let edits = diff(&s, view, &current, blocks)?;
    let revision = request.revision;
    let counts = s
        .memory
        .write(move |store| store.apply_edits(revision, edits, API_ACTOR))
        .await
        .map_err(error)?;
    let snapshot = snapshot(&s).await?;
    Ok(Json(MemoryFileSaved {
        file: render(&s, view, &snapshot)?,
        created: counts.created,
        updated: counts.updated,
        deleted: counts.deleted,
    }))
}

#[derive(Deserialize)]
pub(crate) struct ChangesQuery {
    limit: Option<usize>,
    before: Option<i64>,
}

pub(crate) async fn changes(
    State(s): State<Shared>,
    Query(query): Query<ChangesQuery>,
) -> Result<Json<MemoryChangeList>, HttpError> {
    let limit = query.limit.unwrap_or(50);
    let changes = s
        .memory
        .write(move |store| store.changes(query.before, limit))
        .await
        .map_err(error)?;
    Ok(Json(redacted(&s, changes)?))
}

pub(crate) async fn rebuild(State(s): State<Shared>) -> Result<Json<MemoryRebuild>, HttpError> {
    let (reindexed, embedded) = s
        .memory
        .write(|store| store.rebuild())
        .await
        .map_err(error)?;
    Ok(Json(MemoryRebuild {
        reindexed,
        embedded,
        health: s.memory.health().await,
    }))
}

/// Write the three files as `GET /v1/memory/files/{name}` serves them into
/// `<data_dir>/memory/files` and open that folder on this computer. The folder is a copy:
/// edits there are not read back, the store stays the source of truth. Local access only.
pub(crate) async fn reveal(State(s): State<Shared>) -> Result<StatusCode, HttpError> {
    let snapshot = snapshot(&s).await?;
    let files = MemoryView::ALL
        .into_iter()
        .map(|view| render(&s, view, &snapshot))
        .collect::<Result<Vec<_>, HttpError>>()?;
    let dir = openside_config::data_layout::DataPath::MemoryModel
        .path(&s.config.data_dir)
        .join("files");
    let folder = dir.clone();
    tokio::task::spawn_blocking(move || write_snapshot(&folder, &files))
        .await
        .map_err(|_| HttpError::internal())?
        .map_err(|_| HttpError::internal())?;
    s.memory
        .open_folder(&dir)
        .map_err(|_| HttpError(StatusCode::SERVICE_UNAVAILABLE, "file_manager_unavailable"))?;
    Ok(StatusCode::NO_CONTENT)
}

/// Private to the user like the rest of the data directory: 0700 folder, 0600 files.
fn write_snapshot(dir: &std::path::Path, files: &[MemoryFile]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    for file in files {
        let path = dir.join(&file.name);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            options.mode(0o600);
            // `mode` applies only on create; a copy the user loosened keeps its bits otherwise.
            if path.exists() {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
            }
        }
        std::io::Write::write_all(&mut options.open(&path)?, file.markdown.as_bytes())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_markers_preamble_and_tags() {
        let blocks = parse(
            MemoryView::Memory,
            "# MEMORY\nnew idea\n\n<!-- memory id=mem_a tags=[\"work\",\"a b\"] -->\nfirst\n\n<!-- memory id=mem_b -->\n\n<!-- memory -->\nadded\n",
        )
        .unwrap();
        assert_eq!(
            blocks,
            vec![
                Block {
                    id: None,
                    tags: None,
                    content: "new idea".into()
                },
                Block {
                    id: Some("mem_a".into()),
                    tags: Some(vec!["work".into(), "a b".into()]),
                    content: "first".into()
                },
                Block {
                    id: Some("mem_b".into()),
                    tags: None,
                    content: String::new()
                },
                Block {
                    id: None,
                    tags: None,
                    content: "added".into()
                },
            ]
        );
        assert!(parse(MemoryView::Memory, "<!-- memory id= -->\nx").is_err());
        assert!(parse(MemoryView::Memory, "<!-- memory nope=1 -->\nx").is_err());
        assert!(parse(MemoryView::User, "# USER\n").unwrap().is_empty());
        // A comment that merely starts with the word is content, not a marker.
        assert_eq!(
            parse(MemoryView::Memory, "<!-- memoryless -->")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn note_text_round_trips_and_cannot_forge_markers() {
        let texts = [
            "  Résumé 😀 한국어\r\n\tindented  \r\n\r\nlast line\n\n",
            "<!-- memory -->\nfake split",
            "head\n<!-- memory id=mem_other tags=[\"x\"] -->\n\\<!-- memory -->\n\\\\x",
            "<!-- memory id= -->",
        ];
        let mut markdown = "# MEMORY\n".to_owned();
        for (index, text) in texts.iter().enumerate() {
            markdown.push_str(&format!(
                "\n<!-- memory id=mem_{index} -->\n{}\n",
                escape(&as_shown(text))
            ));
        }
        let blocks = parse(MemoryView::Memory, &markdown).unwrap();
        let parsed: Vec<_> = blocks
            .iter()
            .map(|block| (block.id.as_deref().unwrap(), block.content.as_str()))
            .collect();
        assert_eq!(
            parsed,
            [
                ("mem_0", "Résumé 😀 한국어\n\tindented  \n\nlast line"),
                ("mem_1", "<!-- memory -->\nfake split"),
                (
                    "mem_2",
                    "head\n<!-- memory id=mem_other tags=[\"x\"] -->\n\\<!-- memory -->\n\\\\x"
                ),
                ("mem_3", "<!-- memory id= -->"),
            ]
        );
        for (block, text) in blocks.iter().zip(texts) {
            assert_eq!(block.content, as_shown(text));
        }
    }

    #[test]
    fn markers_carry_escaped_unicode_tags_and_reject_malformed_ones() {
        let blocks = parse(
            MemoryView::User,
            "<!-- memory id=mem_a tags=[\"say \\\"hi\\\"\",\"back\\\\slash\",\"한국어 😀\",\"a-->b\"] -->\ntext",
        )
        .unwrap();
        assert_eq!(
            blocks[0].tags.as_deref().unwrap(),
            ["say \"hi\"", "back\\slash", "한국어 😀", "a-->b"]
        );
        for malformed in [
            "<!-- memory id=a id=b -->",
            "<!-- memory tags=[\"a\"] tags=[\"b\"] -->",
            "<!-- memory tags=[1] -->",
            "<!-- memory tags=nope -->",
            "<!-- memory tags=[\"open -->",
            "<!-- memory id=a extra -->",
        ] {
            assert!(parse(MemoryView::Memory, malformed).is_err(), "{malformed}");
        }
    }
}
