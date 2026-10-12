use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use openside_core::ApprovalTier;
use openside_policy::{Action, Capability};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{parse, spec, DaemonTool, ToolEnv, ToolError, ToolExec};

/// Byte budget for the file content of one `read` page.
#[cfg_attr(not(unix), allow(dead_code))]
const MAX_CONTENT_BYTES: usize = 32 * 1024;
/// Deepest sorted directory window (`offset + limit`) a listing may ask for; bounds memory.
#[cfg_attr(not(unix), allow(dead_code))]
const MAX_LIST_WINDOW: usize = 10_000;

enum ReadError {
    Failed,
    WindowTooLarge,
    Protected,
}

impl From<std::io::Error> for ReadError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::PermissionDenied {
            Self::Protected
        } else {
            Self::Failed
        }
    }
}

#[cfg(test)]
#[path = "files_tests.rs"]
mod tests;

pub(super) fn canonical(path: &str, ctx: &ToolEnv, create: bool) -> Result<PathBuf, ToolError> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        ctx.workspace
            .as_ref()
            .ok_or_else(|| ToolError("Relative paths require a configured workspace".into()))?
            .join(path)
    };
    if ctx.shared.credential_paths.is_protected(&path) {
        return Err(ToolError("Credential source files are protected".into()));
    }
    let resolved = match std::fs::canonicalize(&path) {
        Ok(path) => Ok(path),
        Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
            let parent = path
                .parent()
                .ok_or_else(|| ToolError("path needs an existing parent directory".into()))?;
            let name = path
                .file_name()
                .ok_or_else(|| ToolError("path needs a file name".into()))?;
            // A dangling symlink is not a new file: do not follow its unchecked target.
            if std::fs::symlink_metadata(&path).is_ok() {
                return Err(ToolError("Dangling symlink targets are not allowed".into()));
            }
            Ok(std::fs::canonicalize(parent)
                .map_err(|_| ToolError("path needs an existing parent directory".into()))?
                .join(name))
        }
        Err(_) => Err(ToolError("The requested path is unavailable".into())),
    }?;
    if ctx.shared.credential_paths.is_protected(&resolved) {
        return Err(ToolError("Credential source files are protected".into()));
    }
    Ok(resolved)
}

fn action(capability: Capability, path: PathBuf) -> Vec<Action> {
    vec![Action::new(capability, path.to_string_lossy().into_owned())]
}
fn failure() -> ToolExec {
    ToolExec::Failed("File operation failed; check the path and permissions".into())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadArgs {
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

pub(crate) struct Read;
impl DaemonTool for Read {
    fn name(&self) -> &'static str {
        "read"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<ReadArgs>(self.name(), "Read UTF-8 file lines or directory entries. offset is zero-based; limit defaults to 200. Use offset/limit to fetch additional output.", ApprovalTier::Read, false)
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: ReadArgs = parse(args)?;
            Ok(action(
                Capability::FsRead,
                canonical(&args.path, ctx, false)?,
            ))
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"path":args["path"],"offset":args["offset"],"limit":args["limit"]})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let Ok(args) = parse::<ReadArgs>(&args) else {
                return failure();
            };
            let Ok(path) = ctx.target(Capability::FsRead) else {
                return failure();
            };
            let path = path.to_owned();
            let offset = args.offset.unwrap_or(0);
            let limit = args.limit.unwrap_or(200).min(2000);
            let protected = ctx.shared.credential_paths.clone();
            match tokio::task::spawn_blocking(move || {
                let mut value = nofollow::read(&path, offset, limit, &protected)?;
                if let Some(entries) = value.get_mut("entries").and_then(Value::as_array_mut) {
                    entries.retain(|name| !name.as_str().is_some_and(|name| protected.is_protected(&path.join(name))));
                }
                Ok::<_, ReadError>(value)
            }).await {
                Ok(Ok(for_model)) => ToolExec::Done {
                    for_model,
                    for_app: None,
                },
                Ok(Err(ReadError::WindowTooLarge)) => ToolExec::Denied {
                    reason: "offset + limit is too large for a directory listing".into(),
                    hint: Some(format!(
                        "Directory listings are limited to {MAX_LIST_WINDOW} entries from the start; use a smaller offset"
                    )),
                },
                Ok(Err(ReadError::Protected)) => ToolExec::Denied { reason: "Credential source and process/kernel files are protected".into(), hint: None },
                _ => failure(),
            }
        })
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct WriteArgs {
    path: String,
    content: String,
}

pub(crate) struct Write;
impl DaemonTool for Write {
    fn name(&self) -> &'static str {
        "write"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<WriteArgs>(
            self.name(),
            "Create or replace a UTF-8 file under an existing parent directory.",
            ApprovalTier::Write,
            false,
        )
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: WriteArgs = parse(args)?;
            Ok(action(
                Capability::FsWrite,
                canonical(&args.path, ctx, true)?,
            ))
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"path":args["path"],"content":"[REDACTED]"})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let Ok(args) = parse::<WriteArgs>(&args) else {
                return failure();
            };
            let Ok(path) = ctx.target(Capability::FsWrite) else {
                return failure();
            };
            let path = path.to_owned();
            let bytes = args.content.len();
            let content = args.content;
            let protected = ctx.shared.credential_paths.clone();
            match tokio::task::spawn_blocking(move || {
                nofollow::write(&path, content.as_bytes(), &protected)
            })
            .await
            {
                Ok(Ok(())) => ToolExec::Done {
                    for_model: json!({"written":true,"bytes":bytes}),
                    for_app: None,
                },
                _ => failure(),
            }
        })
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditArgs {
    path: String,
    old_text: String,
    new_text: String,
}

pub(crate) struct Edit;
impl DaemonTool for Edit {
    fn name(&self) -> &'static str {
        "edit"
    }
    fn spec(&self) -> openside_core::ToolSpec {
        spec::<EditArgs>(self.name(), "Replace one exact nonempty old_text occurrence in a UTF-8 file. Ambiguous matches are rejected.", ApprovalTier::Write, false)
    }
    fn visible(&self, _: &ToolEnv) -> bool {
        true
    }
    fn actions<'a>(
        &'a self,
        args: &'a Value,
        ctx: &'a ToolEnv,
    ) -> BoxFuture<'a, Result<Vec<Action>, ToolError>> {
        Box::pin(async move {
            let args: EditArgs = parse(args)?;
            if args.old_text.is_empty() {
                return Err(ToolError("old_text must not be empty".into()));
            }
            Ok(action(
                Capability::FsWrite,
                canonical(&args.path, ctx, false)?,
            ))
        })
    }
    fn redact_args(&self, args: &Value) -> Value {
        json!({"path":args["path"],"old_text":"[REDACTED]","new_text":"[REDACTED]"})
    }
    fn call<'a>(&'a self, args: Value, ctx: &'a ToolEnv) -> BoxFuture<'a, ToolExec> {
        Box::pin(async move {
            let Ok(args) = parse::<EditArgs>(&args) else {
                return failure();
            };
            let Ok(path) = ctx.target(Capability::FsWrite) else {
                return failure();
            };
            let path = path.to_owned();
            let protected = ctx.shared.credential_paths.clone();
            let outcome = tokio::task::spawn_blocking(move || {
                nofollow::edit(&path, &args.old_text, &args.new_text, &protected)
            })
            .await;
            match outcome {
                Ok(Ok(true)) => ToolExec::Done {
                    for_model: json!({"edited":true}),
                    for_app: None,
                },
                Ok(Ok(false)) => ToolExec::Denied {
                    reason: "old_text must match exactly once".into(),
                    hint: Some("Read the file and choose a unique exact match".into()),
                },
                _ => failure(),
            }
        })
    }
}

/// Execution-time file access. Policy approves a canonical path, but a component may be swapped
/// for a symlink before we act on it. Every operation therefore walks the path one component at
/// a time from `/` with `O_NOFOLLOW` and works only through the resulting descriptors, so a
/// swapped component fails the operation instead of redirecting it.
#[cfg(unix)]
mod nofollow {
    use std::{
        collections::BinaryHeap,
        ffi::OsStr,
        fs::File,
        io::{self, BufRead, BufReader, Read, Write},
        os::fd::OwnedFd,
        path::{Component, Path},
    };

    use rustix::fs::{self, AtFlags, Dir, FileType, Mode, OFlags, CWD};
    use serde_json::{json, Value};

    use super::{ReadError, MAX_CONTENT_BYTES, MAX_LIST_WINDOW};
    use crate::credential_paths::ProtectedPaths;

    const TRUNCATION_MARKER: &str = " [line truncated]";

    fn invalid(message: &'static str) -> io::Error {
        io::Error::new(io::ErrorKind::InvalidInput, message)
    }

    fn open_dir(path: &Path) -> io::Result<OwnedFd> {
        if !path.is_absolute() {
            return Err(invalid("path must be absolute"));
        }
        let mut dir = fs::openat(
            CWD,
            "/",
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)?;
        for component in path.components() {
            match component {
                Component::RootDir => {}
                Component::Normal(name) => {
                    dir = fs::openat(
                        &dir,
                        name,
                        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                        Mode::empty(),
                    )
                    .map_err(io::Error::from)?;
                }
                _ => return Err(invalid("path must be canonical")),
            }
        }
        Ok(dir)
    }

    fn split(path: &Path) -> io::Result<(OwnedFd, &OsStr)> {
        let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(invalid("path needs a parent directory and a file name"));
        };
        Ok((open_dir(parent)?, name))
    }

    fn open_file(parent: &OwnedFd, name: &OsStr) -> io::Result<OwnedFd> {
        // NONBLOCK keeps a FIFO from stalling the open; it has no effect on regular files.
        fs::openat(
            parent,
            name,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map_err(io::Error::from)
    }

    fn regular_mode(stat: &fs::Stat) -> io::Result<Mode> {
        if FileType::from_raw_mode(stat.st_mode as _) != FileType::RegularFile {
            return Err(invalid("not a regular file"));
        }
        Ok(Mode::from_raw_mode(stat.st_mode as _))
    }

    pub(super) fn read(
        path: &Path,
        offset: usize,
        limit: usize,
        protected: &ProtectedPaths,
    ) -> Result<Value, ReadError> {
        let fd = if path.file_name().is_none() {
            open_dir(path)?
        } else {
            let (parent, name) = split(path)?;
            open_file(&parent, name)?
        };
        protected.check_fd(&fd)?;
        let stat = fs::fstat(&fd).map_err(io::Error::from)?;
        match FileType::from_raw_mode(stat.st_mode as _) {
            FileType::Directory => list(fd, offset, limit),
            FileType::RegularFile => read_lines(File::from(fd), offset, limit),
            _ => Err(ReadError::Failed),
        }
    }

    fn list(fd: OwnedFd, offset: usize, limit: usize) -> Result<Value, ReadError> {
        let window = offset
            .checked_add(limit)
            .filter(|window| *window <= MAX_LIST_WINDOW)
            .ok_or(ReadError::WindowTooLarge)?;
        let mut dir = Dir::new(fd).map_err(io::Error::from)?;
        // Keep only the window + 1 smallest names (the extra one detects `more`) rather than
        // collecting a whole, possibly huge, directory.
        let mut smallest: BinaryHeap<Vec<u8>> = BinaryHeap::new();
        while let Some(entry) = dir.read() {
            let entry = entry.map_err(io::Error::from)?;
            let name = entry.file_name().to_bytes();
            if name == b"." || name == b".." {
                continue;
            }
            if smallest.len() <= window {
                smallest.push(name.to_vec());
            } else if smallest
                .peek()
                .is_some_and(|largest| name < largest.as_slice())
            {
                smallest.pop();
                smallest.push(name.to_vec());
            }
        }
        let sorted = smallest.into_sorted_vec();
        let more = sorted.len() > window;
        let entries: Vec<_> = sorted
            .iter()
            .skip(offset)
            .take(limit)
            .map(|name| String::from_utf8_lossy(name).into_owned())
            .collect();
        Ok(json!({"entries":entries,"offset":offset,"more":more}))
    }

    fn page(content: String, offset: usize, next: usize, more: bool, truncated: bool) -> Value {
        let mut page = json!({"content":content,"offset":offset,"next_offset":next,"more":more});
        if truncated {
            page["truncated"] = json!(true);
        }
        page
    }

    fn read_lines(file: File, offset: usize, limit: usize) -> Result<Value, ReadError> {
        let mut reader = BufReader::new(file);
        let mut content = String::new();
        let mut line = Vec::new();
        let mut index = 0;
        let mut count = 0;
        let mut truncated = false;
        loop {
            let in_page = index >= offset;
            if in_page && count == limit {
                let more = !reader.fill_buf()?.is_empty();
                return Ok(page(content, offset, index, more, truncated));
            }
            line.clear();
            // A page-wide cap bounds the allocation however long the line is; lines before
            // `offset` are streamed through and dropped.
            let cap = if in_page { MAX_CONTENT_BYTES - 1 } else { 0 };
            let Some(cut) = read_line_capped(&mut reader, &mut line, cap)? else {
                return Ok(page(content, offset, index, false, truncated));
            };
            if !in_page {
                index += 1;
                continue;
            }
            if cut {
                // Make room for the marker so the page never exceeds the budget.
                line.truncate(MAX_CONTENT_BYTES - 1 - TRUNCATION_MARKER.len());
            }
            let marker = if cut { TRUNCATION_MARKER.len() } else { 0 };
            if count > 0 && content.len() + line.len() + marker + 1 > MAX_CONTENT_BYTES {
                // Leave it for the next page, which gives it a fresh budget.
                return Ok(page(content, offset, index, true, truncated));
            }
            content.push_str(decode(&line, cut)?);
            if cut {
                content.push_str(TRUNCATION_MARKER);
                truncated = true;
            }
            content.push('\n');
            count += 1;
            index += 1;
        }
    }

    fn decode(line: &[u8], cut: bool) -> io::Result<&str> {
        match std::str::from_utf8(line) {
            Ok(text) => Ok(text),
            // A cut may land inside a multi-byte character; only that tail is dropped.
            Err(error) if cut && error.error_len().is_none() => {
                std::str::from_utf8(&line[..error.valid_up_to()])
                    .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
            }
            Err(error) => Err(io::Error::new(io::ErrorKind::InvalidData, error)),
        }
    }

    /// Appends one line, without its terminator, to `buf`, keeping at most `cap` bytes; the rest
    /// of the line is consumed and dropped. `None` means EOF, otherwise the flag says whether
    /// the line was cut.
    fn read_line_capped(
        reader: &mut impl BufRead,
        buf: &mut Vec<u8>,
        cap: usize,
    ) -> io::Result<Option<bool>> {
        let mut seen = false;
        let mut cut = false;
        loop {
            let chunk = match reader.fill_buf() {
                Ok(chunk) => chunk,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            };
            if chunk.is_empty() {
                break;
            }
            seen = true;
            let newline = chunk.iter().position(|byte| *byte == b'\n');
            let end = newline.unwrap_or(chunk.len());
            let keep = end.min(cap - buf.len());
            buf.extend_from_slice(&chunk[..keep]);
            cut |= keep < end;
            reader.consume(end + usize::from(newline.is_some()));
            if newline.is_some() {
                if !cut && buf.last() == Some(&b'\r') {
                    buf.pop();
                }
                break;
            }
        }
        Ok(seen.then_some(cut))
    }

    pub(super) fn write(path: &Path, bytes: &[u8], protected: &ProtectedPaths) -> io::Result<()> {
        let (parent, name) = split(path)?;
        protected.check_fd(&parent)?;
        #[cfg(target_os = "linux")]
        let flags = OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        #[cfg(not(target_os = "linux"))]
        let flags = OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mode = match fs::openat(&parent, name, flags, Mode::empty()) {
            Ok(fd) => {
                protected.check_fd(&fd)?;
                Some(regular_mode(&fs::fstat(fd).map_err(io::Error::from)?)?)
            }
            Err(rustix::io::Errno::NOENT) => None,
            Err(error) => return Err(error.into()),
        };
        stage(&parent, name, mode, bytes)
    }

    /// `Ok(false)` means `old` does not occur exactly once.
    pub(super) fn edit(
        path: &Path,
        old: &str,
        new: &str,
        protected: &ProtectedPaths,
    ) -> io::Result<bool> {
        let (parent, name) = split(path)?;
        let fd = open_file(&parent, name)?;
        protected.check_fd(&fd)?;
        let opened = fs::fstat(&fd).map_err(io::Error::from)?;
        let mode = regular_mode(&opened)?;
        let mut content = String::new();
        File::from(fd).read_to_string(&mut content)?;
        if content.match_indices(old).take(2).count() != 1 {
            return Ok(false);
        }
        // Do not replace a different file that took this name after we read the original.
        let current =
            fs::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW).map_err(io::Error::from)?;
        if (current.st_dev, current.st_ino) != (opened.st_dev, opened.st_ino) {
            return Err(invalid("file changed while editing"));
        }
        stage(
            &parent,
            name,
            Some(mode),
            content.replacen(old, new, 1).as_bytes(),
        )?;
        Ok(true)
    }

    /// Replaces `name` atomically via a temp file in the same directory. `rename` never follows
    /// a symlink at the destination, so a swapped final component is replaced, not traversed.
    fn stage(parent: &OwnedFd, name: &OsStr, mode: Option<Mode>, bytes: &[u8]) -> io::Result<()> {
        let (temp_name, temp) = create_temp(parent)?;
        let staged: io::Result<()> = (|| {
            if let Some(mode) = mode {
                fs::fchmod(&temp, mode)?;
            }
            let mut file = File::from(temp);
            file.write_all(bytes)?;
            file.sync_all()?;
            fs::renameat(parent, temp_name.as_str(), parent, name)?;
            Ok(())
        })();
        if staged.is_err() {
            let _ = fs::unlinkat(parent, temp_name.as_str(), AtFlags::empty());
        }
        staged
    }

    fn create_temp(parent: &OwnedFd) -> io::Result<(String, OwnedFd)> {
        for _ in 0..8 {
            let name = format!(".openside-tmp-{:016x}", rand::random::<u64>());
            match fs::openat(
                parent,
                name.as_str(),
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_raw_mode(0o666),
            ) {
                Ok(fd) => return Ok((name, fd)),
                Err(errno) => {
                    let error = io::Error::from(errno);
                    if error.kind() != io::ErrorKind::AlreadyExists {
                        return Err(error);
                    }
                }
            }
        }
        Err(io::Error::from(io::ErrorKind::AlreadyExists))
    }
}

/// Handle-based access needs a Unix descriptor API; the daemon does not serve elsewhere.
#[cfg(not(unix))]
mod nofollow {
    use std::{io, path::Path};

    use serde_json::Value;

    use super::ReadError;

    pub(super) fn read(
        _: &Path,
        _: usize,
        _: usize,
        _: &crate::credential_paths::ProtectedPaths,
    ) -> Result<Value, ReadError> {
        Err(ReadError::Failed)
    }
    pub(super) fn write(
        _: &Path,
        _: &[u8],
        _: &crate::credential_paths::ProtectedPaths,
    ) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
    pub(super) fn edit(
        _: &Path,
        _: &str,
        _: &str,
        _: &crate::credential_paths::ProtectedPaths,
    ) -> io::Result<bool> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
