use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::samples::collect_eml_files;

/// Parse every `.eml` under `path` into its normalized form.
pub fn load(path: &Path) -> Result<Vec<(PathBuf, txp::NormalizedEmail)>> {
    let files = collect_eml_files(path)?;
    if files.is_empty() {
        bail!("no .eml files under {}", path.display());
    }

    let mut emails = Vec::with_capacity(files.len());
    for file in files {
        let bytes = std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
        let email = txp::parse_eml(&bytes, Some(file.display().to_string()))
            .map_err(|error| anyhow::anyhow!("{}: {error}", file.display()))?;
        emails.push((file, email));
    }
    Ok(emails)
}

pub fn render_text(emails: &[(PathBuf, txp::NormalizedEmail)]) -> String {
    let mut out = String::new();

    for (index, (file, email)) in emails.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(&format!("file:    {}\n", file.display()));
        out.push_str(&format!(
            "from:    {} <{}>\n",
            email.from.name.as_deref().unwrap_or(""),
            email.from.email
        ));
        out.push_str(&format!("subject: {}\n", email.subject));
        out.push_str(&format!("date:    {}\n", email.date.to_rfc3339()));
        out.push_str(&format!(
            "source:  {}\n",
            email.source.message_id.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!("blocks ({}):\n", email.blocks.len()));
        for (block_index, block) in email.blocks.iter().enumerate() {
            out.push_str(&format!("{block_index:>3}  {}\n", block.text));
        }
    }

    out
}

pub fn render_json(emails: &[(PathBuf, txp::NormalizedEmail)]) -> Result<String> {
    let values: Vec<serde_json::Value> = emails
        .iter()
        .map(|(file, email)| {
            let mut value = serde_json::to_value(email)?;
            value["file"] = serde_json::Value::String(file.display().to_string());
            Ok(value)
        })
        .collect::<Result<_>>()?;

    Ok(serde_json::to_string_pretty(&values)?)
}
