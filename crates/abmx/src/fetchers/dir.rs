use std::path::PathBuf;

use abmx_core::{Error, FetchBatch, FetchRequest, Fetcher, RawMessage, Result};

/// Read `.eml` files from a directory (or a single file).
///
/// This is the offline path: export messages, put them in a directory, and give
/// them the same `abmx:*` label they would carry in Gmail. Re-scanning is cheap
/// and the state database filters what has already been delivered.
pub struct DirFetcher {
    root: PathBuf,
    label: String,
}

impl DirFetcher {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            label: "abmx:dir".to_owned(),
        }
    }

    pub fn labeled(root: impl Into<PathBuf>, label: impl Into<String>) -> Self {
        Self {
            root: root.into(),
            label: label.into(),
        }
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn files(&self) -> Result<Vec<PathBuf>> {
        if self.root.is_file() {
            return Ok(vec![self.root.clone()]);
        }

        if !self.root.exists() {
            return Err(Error::Fetch(format!(
                "{} does not exist",
                self.root.display()
            )));
        }

        let mut files = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|error| Error::io(&self.root, error))? {
            let entry = entry.map_err(|error| Error::io(&self.root, error))?;
            let path = entry.path();
            let is_eml = path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("eml"))
                .unwrap_or(false);
            if is_eml && path.is_file() {
                files.push(path);
            }
        }

        files.sort();
        Ok(files)
    }
}

impl Fetcher for DirFetcher {
    fn name(&self) -> &str {
        "dir"
    }

    fn fetch(&self, request: FetchRequest<'_>) -> Result<FetchBatch> {
        let files = self.files()?;
        let mut messages = Vec::with_capacity(files.len());

        for path in files.iter().take(request.limit) {
            let bytes = std::fs::read(path).map_err(|error| Error::io(path, error))?;
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| path.display().to_string());

            messages.push(RawMessage::new(
                format!("dir:{name}"),
                self.label.clone(),
                path.display().to_string(),
                bytes,
            ));
        }

        Ok(FetchBatch {
            skipped: files.len().saturating_sub(messages.len()),
            messages,
        })
    }
}
