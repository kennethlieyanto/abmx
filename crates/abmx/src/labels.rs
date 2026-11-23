//! Deciding what a set of discovered Gmail labels means for abmx.
//!
//! `[receipts.accounts]` and `[statements.accounts]` are the only source of
//! truth for what abmx manages. A label under the prefix that has no account
//! entry is *unmanaged*: `abmx status` reports it as a failure to fix, and
//! `abmx setup` offers to map or ignore it. `[receipts].ignore` and
//! `[statements].ignore` remember an explicit "no" so status stays quiet.

use std::collections::BTreeSet;

use crate::config::{ReceiptsSection, StatementsSection};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLabel {
    /// The Gmail label exactly as Gmail spells it.
    pub label: String,
    /// The decoder ring: the remainder after the prefix.
    pub parser_id: String,
    /// The configured account (the first one for a `parser:section` key).
    pub account: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredLabel {
    pub label: String,
    pub parser_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LabelPlan {
    /// Labels sync should fetch.
    pub managed: Vec<ManagedLabel>,
    /// Valid labels the user has not mapped and has not declined.
    pub unmanaged: Vec<DiscoveredLabel>,
    /// Valid labels the user declined in `abmx setup`.
    pub ignored: Vec<DiscoveredLabel>,
    /// Labels under the prefix whose remainder is not a parser id.
    pub invalid: Vec<String>,
    /// Configured parser ids with no matching Gmail label.
    pub orphans: Vec<String>,
}

impl LabelPlan {
    pub fn managed_names(&self) -> Vec<String> {
        self.managed
            .iter()
            .map(|label| label.label.clone())
            .collect()
    }

    /// Labels that are neither mapped nor declined. Setup asks about these.
    pub fn needs_setup(&self) -> bool {
        !self.unmanaged.is_empty() || !self.invalid.is_empty()
    }
}

/// Classify labels discovered under a receipt prefix.
pub fn plan(discovered: &[String], labels: &ReceiptsSection) -> LabelPlan {
    plan_with(
        discovered,
        |label| labels.parser_id(label),
        |parser_id| labels.is_ignored(parser_id),
        |parser_id| labels.accounts.get(parser_id).cloned(),
        labels.accounts.keys().cloned(),
    )
}

/// Classify labels discovered under a statement prefix.
///
/// Statement keys may be `parser:section`, so a mapping is matched by the
/// parser id before the colon, and several keys can point at the same label.
pub fn plan_statements(discovered: &[String], labels: &StatementsSection) -> LabelPlan {
    plan_with(
        discovered,
        |label| labels.parser_id(label),
        |parser_id| labels.is_ignored(parser_id),
        |parser_id| {
            labels
                .accounts
                .iter()
                .find(|(key, _)| key_parser_id(key) == parser_id)
                .map(|(_, account)| account.clone())
        },
        labels
            .accounts
            .keys()
            .map(|key| key_parser_id(key).to_owned()),
    )
}

/// The parser id part of a `[statements.accounts]` key (`parser` or
/// `parser:section`).
pub fn key_parser_id(key: &str) -> &str {
    key.split_once(':').map_or(key, |(parser, _)| parser)
}

fn plan_with(
    discovered: &[String],
    parse: impl Fn(&str) -> Option<String>,
    is_ignored: impl Fn(&str) -> bool,
    account_for: impl Fn(&str) -> Option<String>,
    mapped_keys: impl Iterator<Item = String>,
) -> LabelPlan {
    let mut plan = LabelPlan::default();
    let mut seen = BTreeSet::new();

    for label in discovered {
        let Some(parser_id) = parse(label) else {
            plan.invalid.push(label.clone());
            continue;
        };

        if !seen.insert(parser_id.clone()) {
            // `Receipt/x` and `receipt/x` are the same parser id: label matching
            // is case-insensitive. Gmail lets both exist, but abmx routes by
            // parser id; keep the first.
            continue;
        }

        if let Some(account) = account_for(&parser_id) {
            plan.managed.push(ManagedLabel {
                label: label.clone(),
                parser_id,
                account,
            });
        } else if is_ignored(&parser_id) {
            plan.ignored.push(DiscoveredLabel {
                label: label.clone(),
                parser_id,
            });
        } else {
            plan.unmanaged.push(DiscoveredLabel {
                label: label.clone(),
                parser_id,
            });
        }
    }

    let mut orphaned = BTreeSet::new();
    for parser_id in mapped_keys {
        if !seen.contains(&parser_id) && orphaned.insert(parser_id.clone()) {
            plan.orphans.push(parser_id);
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> ReceiptsSection {
        ReceiptsSection {
            enabled: true,
            prefix: "receipt/".to_owned(),
            since: None,
            accounts: [
                ("example.card_debit".to_owned(), "uuid-example".to_owned()),
                ("example.qris_payment".to_owned(), "uuid-wallet".to_owned()),
            ]
            .into_iter()
            .collect(),
            ignore: vec!["merchant.promo".to_owned()],
        }
    }

    fn statements() -> StatementsSection {
        StatementsSection {
            enabled: true,
            prefix: "financial_statements/".to_owned(),
            since: None,
            accounts: [
                (
                    "example.consolidated_statement".to_owned(),
                    "uuid-main".to_owned(),
                ),
                (
                    "example.consolidated_statement:savings".to_owned(),
                    "uuid-savings".to_owned(),
                ),
            ]
            .into_iter()
            .collect(),
            ignore: vec!["example.portfolio_summary".to_owned()],
            passwords: Default::default(),
        }
    }

    fn discovered(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_default_prefix_is_receipt_slash() {
        assert_eq!(ReceiptsSection::default().prefix, "receipt/");
    }

    #[test]
    fn splits_managed_unmanaged_ignored_invalid_and_orphans() {
        let plan = plan(
            &discovered(&[
                "receipt/example.card_debit",
                "receipt/new.parser",
                "receipt/merchant.promo",
                "receipt/bad/name",
            ]),
            &labels(),
        );

        assert_eq!(plan.managed.len(), 1);
        assert_eq!(plan.managed[0].account, "uuid-example");
        assert_eq!(plan.unmanaged.len(), 1);
        assert_eq!(plan.unmanaged[0].parser_id, "new.parser");
        assert_eq!(plan.ignored.len(), 1);
        assert_eq!(plan.invalid, vec!["receipt/bad/name"]);
        assert_eq!(plan.orphans, vec!["example.qris_payment"]);
    }

    #[test]
    fn ignored_labels_are_case_insensitive() {
        let plan = plan(&discovered(&["receipt/Merchant.Promo"]), &labels());
        assert_eq!(plan.ignored.len(), 1);
        assert!(plan.unmanaged.is_empty());
    }

    #[test]
    fn duplicate_parser_ids_are_kept_once() {
        let plan = plan(
            &discovered(&[
                "receipt/example.card_debit",
                "Receipt/example.card_debit",
                "receipt/example.qris_payment",
            ]),
            &labels(),
        );
        assert_eq!(plan.managed.len(), 2);
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn a_fully_mapped_inventory_has_nothing_to_do() {
        let plan = plan(
            &discovered(&["receipt/example.card_debit", "receipt/example.qris_payment"]),
            &labels(),
        );
        assert_eq!(plan.managed.len(), 2);
        assert!(plan.unmanaged.is_empty());
        assert!(plan.ignored.is_empty());
        assert!(plan.invalid.is_empty());
        assert!(plan.orphans.is_empty());
        assert!(!plan.needs_setup());
    }

    #[test]
    fn statements_map_through_section_keys() {
        let plan = plan_statements(
            &discovered(&[
                "financial_statements/example.consolidated_statement",
                "financial_statements/example.portfolio_summary",
                "financial_statements/new.bank",
            ]),
            &statements(),
        );

        assert_eq!(plan.managed.len(), 1);
        assert_eq!(plan.managed[0].parser_id, "example.consolidated_statement");
        assert_eq!(
            plan.managed[0].label,
            "financial_statements/example.consolidated_statement"
        );
        assert_eq!(plan.ignored.len(), 1);
        assert_eq!(plan.unmanaged.len(), 1);
        assert_eq!(plan.unmanaged[0].parser_id, "new.bank");
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn statement_orphans_are_deduped_by_parser_id() {
        let plan = plan_statements(
            &discovered(&["financial_statements/example.portfolio_summary"]),
            &statements(),
        );

        assert_eq!(plan.orphans, vec!["example.consolidated_statement"]);
    }
}
