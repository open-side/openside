use crate::{error, ConfigError};
use std::path::{Path, PathBuf};

pub(crate) fn resolve(paths: &mut [PathBuf], config: &Path) -> Result<(), ConfigError> {
    for path in paths {
        let Some(text) = path.to_str() else {
            return Err(error(
                config,
                "browser.extensions",
                "extension paths must be UTF-8",
            ));
        };
        if text.is_empty() || text.contains([',', '\0']) {
            return Err(error(
                config,
                "browser.extensions",
                "extension paths must be non-empty and contain no commas or NUL bytes",
            ));
        }
        if path.is_relative() {
            *path = config
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(&*path);
        }
        *path = std::fs::canonicalize(&*path).map_err(|_| {
            error(
                config,
                "browser.extensions",
                "unpacked extension directory does not exist or cannot be inspected",
            )
        })?;
        if !path.is_dir() {
            return Err(error(
                config,
                "browser.extensions",
                "unpacked extension path must be a directory",
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate(paths: &[PathBuf]) -> Result<(), &'static str> {
    if paths.iter().any(|path| {
        !path.is_absolute()
            || !path.is_dir()
            || path
                .to_str()
                .map_or(true, |text| text.contains([',', '\0']))
    }) {
        Err("provide existing absolute unpacked extension directories without commas or NUL bytes")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    #[test]
    fn doctor_browser_extensions_resolve_relative_to_config_and_reject_flag_delimiters() {
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config/openside/config.toml");
        let extension = config.parent().unwrap().join("extension with spaces");
        std::fs::create_dir_all(&extension).unwrap();
        let parsed =
            crate::parse_document(&config, "[browser]\nextensions = ['extension with spaces']")
                .unwrap();
        assert_eq!(
            parsed.browser.extensions,
            vec![extension.canonicalize().unwrap()]
        );
        assert!(crate::parse_document(&config, "[browser]\nextensions = ['missing']").is_err());
        assert!(crate::parse_document(
            &config,
            "[browser]\nextensions = ['ext,--remote-debugging-address=bad']"
        )
        .is_err());
        assert!(crate::parse_document(&config, "[browser]\nextensions = ['']").is_err());
        assert!(crate::Config::default().browser.extensions.is_empty());
    }
}
