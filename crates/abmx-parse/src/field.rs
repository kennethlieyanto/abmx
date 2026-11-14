//! Selectors: turn a labelled region into a single string.

use regex::Regex;

use crate::{Field, Normalize, Regions};

#[derive(Debug, Clone)]
pub struct CompiledField {
    pub spec: Field,
    pub regex: Option<Regex>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("label `{0}` not present in this template")]
    MissingLabel(String),
}

impl CompiledField {
    pub fn get(&self, regions: &Regions<'_>) -> Result<Option<String>, ExtractError> {
        let blocks = regions
            .get(&self.spec.label)
            .ok_or_else(|| ExtractError::MissingLabel(self.spec.label.clone()))?;

        let value = if let Some(index) = self.spec.block {
            // A fixed block may still need a regex to pull the wanted part out
            // of it (e.g. "Bank Example - 1234567890123" -> the account number).
            blocks.get(index).and_then(|block| match &self.regex {
                Some(regex) => captures(regex, block),
                None => Some((*block).to_string()),
            })
        } else if self.spec.per_block {
            match &self.regex {
                Some(regex) => blocks.iter().find_map(|block| captures(regex, block)),
                None => blocks.first().map(|value| (*value).to_string()),
            }
        } else {
            let joined = blocks.join(" ");
            match &self.regex {
                Some(regex) => captures(regex, &joined),
                None => Some(joined),
            }
        };

        Ok(value.map(|value| self.apply_normalize(value)))
    }

    fn apply_normalize(&self, value: String) -> String {
        match self.spec.normalize {
            Some(Normalize::StripSpaces) => {
                value.chars().filter(|ch| !ch.is_whitespace()).collect()
            }
            None => value,
        }
    }
}

fn captures(regex: &Regex, haystack: &str) -> Option<String> {
    let captures = regex.captures(haystack)?;

    // Prefer the first named group (`value`, `last4`, ...) so that a selector can
    // constrain its match without a second regex.
    for name in regex.capture_names().flatten() {
        if let Some(found) = captures.name(name) {
            return Some(found.as_str().to_string());
        }
    }

    captures
        .get(1)
        .or_else(|| captures.get(0))
        .map(|found| found.as_str().to_string())
}
