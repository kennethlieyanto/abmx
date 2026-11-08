//! Routing: which parser owns this email?
//!
//! `parser.toml` owns the sender and subject, so the router is *derived* and a
//! new parser can never be forgotten here.  `router.toml` only holds policy:
//! ignore rules and overrides. What to do with unmatched mail is the caller's
//! decision, not the library's.

use regex::Regex;
use serde::Deserialize;
use txp_core::NormalizedEmail;

use crate::{norm, Parser};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct RouterConfig {
    #[serde(default)]
    pub router: Policy,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub ignore: IgnoreRules,
    #[serde(rename = "override")]
    pub overrides: Vec<OverrideRule>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct IgnoreRules {
    /// Exact addresses or `*@domain` patterns.
    pub senders: Vec<String>,
    /// Regexes matched against the subject.
    pub subjects: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OverrideRule {
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub subject: Option<String>,
    pub parser: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteReason {
    Override,
    Unique,
    MostSpecific,
    Ignored,
    NoMatch,
    Ambiguous,
}

impl RouteReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RouteReason::Override => "override",
            RouteReason::Unique => "unique",
            RouteReason::MostSpecific => "most_specific",
            RouteReason::Ignored => "ignored",
            RouteReason::NoMatch => "no_match",
            RouteReason::Ambiguous => "ambiguous",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub id: String,
    pub sender: bool,
    pub subject: bool,
    pub requires_matched: usize,
    pub requires_total: usize,
    pub rejects_clear: bool,
    pub contains_ok: bool,
    pub excludes_ok: bool,
    pub specificity: usize,
}

impl Candidate {
    pub fn eligible(&self) -> bool {
        self.sender
            && self.subject
            && self.requires_matched == self.requires_total
            && self.rejects_clear
            && self.contains_ok
            && self.excludes_ok
    }
}

#[derive(Debug, Clone)]
pub struct Route {
    pub parser: Option<usize>,
    pub reason: RouteReason,
    pub candidates: Vec<Candidate>,
}

impl Route {
    pub fn explain(&self) -> String {
        match self.parser {
            Some(_) => format!("matched ({})", self.reason.as_str()),
            None => format!("unmatched ({})", self.reason.as_str()),
        }
    }
}

pub fn route(parsers: &[Parser], config: &RouterConfig, email: &NormalizedEmail) -> Route {
    let policy = &config.router;

    if is_ignored(policy, email) {
        return Route {
            parser: None,
            reason: RouteReason::Ignored,
            candidates: Vec::new(),
        };
    }

    for rule in &policy.overrides {
        if override_matches(rule, email) {
            if let Some(index) = parsers
                .iter()
                .position(|parser| parser.spec.id == rule.parser)
            {
                return Route {
                    parser: Some(index),
                    reason: RouteReason::Override,
                    candidates: Vec::new(),
                };
            }
        }
    }

    let block_set: std::collections::HashSet<String> =
        email.blocks.iter().map(|block| norm(&block.text)).collect();

    let mut candidates = Vec::with_capacity(parsers.len());
    for parser in parsers {
        if !parser.spec.enabled {
            continue;
        }

        let sender = parser
            .spec
            .matcher
            .senders
            .iter()
            .any(|pattern| sender_matches(pattern, &email.from.email));

        let subject = parser.subject.is_match(&email.subject);

        // A label's presence is a property of the email, not of this spec's
        // declared label list: `rejects` often names a label owned by a sibling
        // template (three example journals share one sender and subject).
        let requires_matched = parser
            .requires
            .iter()
            .filter(|label| block_set.contains(*label))
            .count();
        let rejects_clear = !parser.rejects.iter().any(|label| block_set.contains(label));

        let text_matches =
            |regex: &Regex| email.blocks.iter().any(|block| regex.is_match(&block.text));
        let contains_ok = parser.contains.iter().all(text_matches);
        let excludes_ok = !parser.excludes.iter().any(text_matches);

        candidates.push(Candidate {
            id: parser.spec.id.clone(),
            sender,
            subject,
            requires_matched,
            requires_total: parser.requires.len(),
            rejects_clear,
            contains_ok,
            excludes_ok,
            specificity: requires_matched + parser.contains.len(),
        });
    }

    let eligible: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, candidate)| candidate.eligible())
        .map(|(index, _)| index)
        .collect();

    let (parser, reason) = match eligible.as_slice() {
        [] => (None, RouteReason::NoMatch),
        [only] => (Some(*only), RouteReason::Unique),
        many => {
            let best = many
                .iter()
                .map(|index| candidates[*index].specificity)
                .max()
                .unwrap_or(0);
            let top: Vec<usize> = many
                .iter()
                .copied()
                .filter(|index| candidates[*index].specificity == best)
                .collect();

            match top.as_slice() {
                [only] => (Some(*only), RouteReason::MostSpecific),
                _ => (None, RouteReason::Ambiguous),
            }
        }
    };

    Route {
        parser,
        reason,
        candidates,
    }
}

fn is_ignored(policy: &Policy, email: &NormalizedEmail) -> bool {
    if policy
        .ignore
        .senders
        .iter()
        .any(|pattern| sender_matches(pattern, &email.from.email))
    {
        return true;
    }

    policy.ignore.subjects.iter().any(|pattern| {
        Regex::new(pattern)
            .map(|regex| regex.is_match(&email.subject))
            .unwrap_or(false)
    })
}

fn override_matches(rule: &OverrideRule, email: &NormalizedEmail) -> bool {
    let sender = rule
        .sender
        .as_deref()
        .map(|pattern| sender_matches(pattern, &email.from.email))
        .unwrap_or(true);

    let subject = rule
        .subject
        .as_deref()
        .map(|pattern| {
            Regex::new(pattern)
                .map(|regex| regex.is_match(&email.subject))
                .unwrap_or(false)
        })
        .unwrap_or(true);

    sender && subject
}

/// `alerts@bank.com` (exact, case-insensitive) or `*@bank.com`.
pub fn sender_matches(pattern: &str, email: &str) -> bool {
    let pattern = pattern.trim();
    let email = email.trim();

    if let Some(suffix) = pattern.strip_prefix('*') {
        return email
            .to_ascii_lowercase()
            .ends_with(&suffix.to_ascii_lowercase());
    }

    pattern.eq_ignore_ascii_case(email)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sender_matching() {
        assert!(sender_matches("example@example.invalid", "example@example.invalid"));
        assert!(sender_matches(
            "*@bank.example",
            "receipts@bank.example"
        ));
        assert!(!sender_matches(
            "*@bank.example",
            "receipts@example.com"
        ));
        assert!(!sender_matches("example@example.invalid", "support@bank.example.com"));
    }
}
