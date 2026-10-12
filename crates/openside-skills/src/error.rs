/// Every way a skill can be rejected, read, stored or installed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A document, file or tree exceeds a hard size limit.
    #[error("{what} is larger than {max} bytes")]
    TooLarge {
        /// What was measured.
        what: &'static str,
        /// The limit that was exceeded.
        max: usize,
    },
    /// `SKILL.md` does not begin with a frontmatter block.
    #[error("SKILL.md must start with a `---` frontmatter block")]
    MissingFrontmatter,
    /// The closing `---` line is absent.
    #[error("frontmatter is not closed by a `---` line")]
    UnterminatedFrontmatter,
    /// The frontmatter is not in the supported YAML subset.
    #[error("frontmatter line {line}: {message}")]
    Syntax {
        /// 1-based line in `SKILL.md`.
        line: usize,
        /// What is wrong.
        message: String,
    },
    /// A field is missing or breaks a documented rule.
    #[error("invalid `{field}`: {message}")]
    Invalid {
        /// Frontmatter key (or `body`, `resource`).
        field: String,
        /// What is wrong.
        message: String,
    },
    /// No such skill (or resource).
    #[error("skill `{0}` not found")]
    NotFound(String),
    /// A skill with this name is already stored.
    #[error("skill `{0}` already exists")]
    Exists(String),
    /// The skill lives in a read-only source (project skills).
    #[error("skill `{0}` is read-only")]
    ReadOnly(String),
    /// An install source was refused for safety (symlink escape, executable, special file).
    #[error("{0}")]
    Unsafe(String),
    /// Filesystem failure.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
}

impl Error {
    pub(crate) fn invalid(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.into(),
            message: message.into(),
        }
    }

    pub(crate) fn io(context: impl Into<String>, source: std::io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }

    pub(crate) fn io_path(action: &str, path: &std::path::Path, source: std::io::Error) -> Self {
        Self::io(format!("{action} {}", path.display()), source)
    }
}

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;
