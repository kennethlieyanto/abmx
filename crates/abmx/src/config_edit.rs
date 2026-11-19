//! Format-preserving edits to `abmx.toml`.
//!
//! `abmx setup` writes the same file a human edits, so rewriting it with
//! `toml::to_string` would drop comments, ordering, and fields this build does
//! not know about. `toml_edit` keeps all of that; only the values we touch
//! change.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use tempfile::NamedTempFile;
use toml_edit::{value, Array, DocumentMut, Item, Table, Value};

use crate::config::INIT_TEMPLATE;

/// A config file loaded for editing.
pub struct ConfigDocument {
    path: PathBuf,
    write_path: PathBuf,
    doc: DocumentMut,
    dirty: bool,
}

impl ConfigDocument {
    /// Read the existing file, or start from the starter template when the
    /// path does not exist yet.
    ///
    /// A symlinked config (common for dotfiles) is edited at its target so the
    /// link is not replaced by a regular file.
    pub fn open_or_template(path: &Path) -> Result<Self> {
        let doc = if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            text.parse::<DocumentMut>()
                .with_context(|| format!("parsing {}", path.display()))?
        } else {
            INIT_TEMPLATE
                .parse::<DocumentMut>()
                .context("parsing the starter config template")?
        };

        let write_path = if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            std::fs::canonicalize(path)
                .with_context(|| format!("resolving symlink {}", path.display()))?
        } else {
            path.to_path_buf()
        };

        Ok(Self {
            path: path.to_path_buf(),
            write_path,
            doc,
            dirty: false,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Write the document atomically when anything changed.
    pub fn save(&mut self) -> Result<bool> {
        if !self.dirty {
            return Ok(false);
        }

        atomic_write(
            &self.write_path,
            self.doc.to_string().as_bytes(),
            file_mode(&self.write_path),
        )?;
        self.dirty = false;
        Ok(true)
    }

    pub fn set_string(&mut self, keys: &[&str], input: &str) -> Result<()> {
        self.set(keys, value(input))
    }

    pub fn remove(&mut self, keys: &[&str]) -> Result<bool> {
        let (table, last) = self.table_for(keys)?;
        let removed = table.remove(&last).is_some();
        if removed {
            self.dirty = true;
        }
        Ok(removed)
    }

    /// Point `[fetch.gmail]` at Google's downloaded OAuth client JSON.
    pub fn set_gmail_credentials_file(&mut self, path: &str) -> Result<()> {
        self.set_string(&["fetch", "gmail", "credentials_file"], path)
    }

    /// Set the Actual Budget endpoint and where its secrets live.
    pub fn set_actual(
        &mut self,
        server_url: &str,
        budget_sync_id: &str,
        api_key_file: Option<&str>,
        server_password_file: Option<&str>,
        budget_password_file: Option<&str>,
    ) -> Result<()> {
        self.set_string(&["sink", "actual", "server_url"], server_url)?;
        // A config carried over from actual-http-api still has `url`; drop it
        // so the deprecated field cannot shadow the real server later.
        self.remove(&["sink", "actual", "url"])?;
        self.set_string(&["sink", "actual", "budget_sync_id"], budget_sync_id)?;
        if let Some(path) = api_key_file {
            self.set_string(&["sink", "actual", "api_key_file"], path)?;
        }
        if let Some(path) = server_password_file {
            self.set_string(&["sink", "actual", "server_password_file"], path)?;
        }
        if let Some(path) = budget_password_file {
            self.set_string(&["sink", "actual", "budget_password_file"], path)?;
        }
        Ok(())
    }

    pub fn set_account(&mut self, parser_id: &str, account: &str) -> Result<()> {
        let table = self.table(&["labels", "accounts"])?;
        table.insert(parser_id, value(account));
        self.dirty = true;
        Ok(())
    }

    pub fn remove_account(&mut self, parser_id: &str) -> Result<bool> {
        let table = self.table(&["labels", "accounts"])?;
        let removed = table.remove(parser_id).is_some();
        if removed {
            self.dirty = true;
        }
        Ok(removed)
    }

    /// Remember an explicit "no" from setup. Order is preserved and duplicates
    /// are case-insensitive.
    pub fn add_ignore(&mut self, parser_id: &str) -> Result<()> {
        let array = self.array(&["labels", "ignore"])?;
        let exists = array.iter().any(|item| {
            item.as_str()
                .is_some_and(|existing| existing.eq_ignore_ascii_case(parser_id))
        });
        if !exists {
            array.push(parser_id);
            self.dirty = true;
        }
        Ok(())
    }

    pub fn remove_ignore(&mut self, parser_id: &str) -> Result<bool> {
        let array = self.array(&["labels", "ignore"])?;
        let before = array.len();
        let keep: Vec<Value> = array
            .iter()
            .filter(|item| {
                !item
                    .as_str()
                    .is_some_and(|existing| existing.eq_ignore_ascii_case(parser_id))
            })
            .cloned()
            .collect();
        if keep.len() == before {
            return Ok(false);
        }

        let mut replacement = Array::new();
        for item in keep {
            replacement.push_formatted(item);
        }
        *array = replacement;
        self.dirty = true;
        Ok(true)
    }

    pub fn ignore_entries(&self) -> Vec<String> {
        self.doc
            .get("labels")
            .and_then(Item::as_table)
            .and_then(|labels| labels.get("ignore"))
            .and_then(Item::as_array)
            .map(|array| {
                array
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn set(&mut self, keys: &[&str], item: Item) -> Result<()> {
        let (table, last) = self.table_for(keys)?;
        table.insert(&last, item);
        self.dirty = true;
        Ok(())
    }

    fn table(&mut self, path: &[&str]) -> Result<&mut Table> {
        let (table, last) = self.table_for(path)?;
        let item = table
            .entry(&last)
            .or_insert_with(|| Item::Table(Table::new()));
        item.as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("`{}` is not a TOML table", path.join(".")))
    }

    fn array(&mut self, path: &[&str]) -> Result<&mut Array> {
        let (table, last) = self.table_for(path)?;
        let item = table
            .entry(&last)
            .or_insert_with(|| Item::Value(Value::Array(Array::new())));
        item.as_array_mut()
            .ok_or_else(|| anyhow::anyhow!("`{}` is not a TOML array", path.join(".")))
    }

    /// Walk to the parent table of `keys` and return it with the final key.
    fn table_for(&mut self, keys: &[&str]) -> Result<(&mut Table, String)> {
        let Some((last, parents)) = keys.split_last() else {
            bail!("empty config key path");
        };

        let mut table = self.doc.as_table_mut();
        for key in parents {
            let item = table
                .entry(key)
                .or_insert_with(|| Item::Table(Table::new()));
            table = item
                .as_table_mut()
                .ok_or_else(|| anyhow::anyhow!("`{key}` is not a TOML table"))?;
        }

        Ok((table, (*last).to_owned()))
    }
}

/// Write a secret file with mode 0600. Used for the Actual API key and budget
/// password, which have no standard JSON shape.
pub fn write_secret_file(path: &Path, value: &str) -> Result<()> {
    let mut bytes = value.trim().as_bytes().to_vec();
    bytes.push(b'\n');
    atomic_write(path, &bytes, 0o600)
}

fn file_mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(path) {
            return metadata.permissions().mode() & 0o777;
        }
        0o644
    }
    #[cfg(not(unix))]
    {
        0o644
    }
}

fn atomic_write(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temporary file in {}", parent.display()))?;

    {
        use std::io::Write;
        temp.write_all(bytes)
            .with_context(|| format!("writing {}", path.display()))?;
        temp.flush()
            .with_context(|| format!("writing {}", path.display()))?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))
            .with_context(|| format!("setting permissions on {}", path.display()))?;
    }
    #[cfg(not(unix))]
    let _ = mode;

    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXISTING: &str = r#"# keep this comment
[general]
parsers = []

# and this one
[fetch.gmail]
client_id = "old.apps.googleusercontent.com"
max_messages = 50

[labels]
prefix = "receipt/"

[labels.accounts]
"example.card_debit" = "uuid-old"

[unknown]
future = "field"
"#;

    #[test]
    fn edits_preserve_comments_and_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abmx.toml");
        std::fs::write(&path, EXISTING).unwrap();

        let mut doc = ConfigDocument::open_or_template(&path).unwrap();
        doc.set_gmail_credentials_file("gmail-client.json").unwrap();
        doc.set_account("example.qris_payment", "uuid-new").unwrap();
        doc.set_account("example.card_debit", "uuid-updated")
            .unwrap();
        assert!(doc.save().unwrap());

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# keep this comment"));
        assert!(text.contains("# and this one"));
        assert!(text.contains("[unknown]"));
        assert!(text.contains("future = \"field\""));
        assert!(text.contains("credentials_file = \"gmail-client.json\""));
        assert!(text.contains("max_messages = 50"));
        assert!(text.contains("\"example.qris_payment\" = \"uuid-new\""));
        assert!(text.contains("\"example.card_debit\" = \"uuid-updated\""));
    }

    #[test]
    fn dotted_parser_ids_are_written_as_quoted_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abmx.toml");
        let mut doc = ConfigDocument::open_or_template(&path).unwrap();
        doc.set_account("example.card_debit", "uuid").unwrap();
        doc.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"example.card_debit\" = \"uuid\""),
            "got:\n{text}"
        );
    }

    #[test]
    fn ignore_entries_are_deduped_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abmx.toml");
        let mut doc = ConfigDocument::open_or_template(&path).unwrap();

        doc.add_ignore("Merchant.Promo").unwrap();
        doc.add_ignore("merchant.promo").unwrap();
        assert_eq!(doc.ignore_entries(), vec!["Merchant.Promo"]);

        assert!(doc.remove_ignore("MERCHANT.PROMO").unwrap());
        assert!(doc.ignore_entries().is_empty());
        assert!(!doc.remove_ignore("merchant.promo").unwrap());

        doc.add_ignore("one").unwrap();
        doc.add_ignore("two").unwrap();
        doc.save().unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("ignore = [\"one\", \"two\"]"), "got:\n{text}");
    }

    #[test]
    fn saving_without_changes_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abmx.toml");
        std::fs::write(&path, EXISTING).unwrap();

        let mut doc = ConfigDocument::open_or_template(&path).unwrap();
        assert!(!doc.save().unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), EXISTING);
    }

    #[test]
    fn a_new_file_starts_from_the_template() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/abmx.toml");
        let mut doc = ConfigDocument::open_or_template(&path).unwrap();
        doc.set_gmail_credentials_file("gmail-client.json").unwrap();
        doc.save().unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("abmx configuration"));
        assert!(text.contains("credentials_file = \"gmail-client.json\""));
        // The template's secrets-do-not-live-here guidance survives.
        assert!(text.contains("ABMX_GMAIL_CLIENT_SECRET"));
    }

    #[test]
    fn secret_files_are_private() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets/api-key");
        write_secret_file(&path, "s3cret\n").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[cfg(unix)]
    #[test]
    fn saving_through_a_symlink_keeps_the_link() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles/abmx.toml");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, EXISTING).unwrap();

        let link = dir.path().join("config.toml");
        symlink(&real, &link).unwrap();

        let mut doc = ConfigDocument::open_or_template(&link).unwrap();
        doc.set_gmail_credentials_file("gmail-client.json").unwrap();
        doc.save().unwrap();

        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        let text = std::fs::read_to_string(&real).unwrap();
        assert!(text.contains("credentials_file = \"gmail-client.json\""));
    }
}
