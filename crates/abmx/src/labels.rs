//! Deciding what a set of discovered Gmail labels means for abmx.
//!
//! `[labels.accounts]` is the only source of truth for what abmx manages. A
//! label under the prefix that has no account entry is *unmanaged*, not a
//! configuration error: gmailctl may create labels ahead of time, and the user
//! may simply not want some of them imported. `[labels].ignore` remembers an
//! explicit "no" so `abmx setup` does not ask again.

use std::collections::BTreeSet;

use crate::config::LabelsSection;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedLabel {
    /// The Gmail label exactly as Gmail spells it.
    pub label: String,
    /// The decoder ring: the remainder after the prefix.
    pub parser_id: String,
    /// The `[labels.accounts]` value.
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
    /// `[labels.accounts]` entries with no matching Gmail label.
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

/// Classify labels discovered under the configured prefix.
pub fn plan(discovered: &[String], labels: &LabelsSection) -> LabelPlan {
    let mut plan = LabelPlan::default();
    let mut seen = BTreeSet::new();

    for label in discovered {
        let Some(parser_id) = labels.parser_id(label) else {
            plan.invalid.push(label.clone());
            continue;
        };

        if !seen.insert(parser_id.clone()) {
            // `Receipt/x` and `receipt/x` are the same parser id: label matching
            // is case-insensitive. Gmail lets both exist, but abmx routes by
            // parser id; keep the first.
            continue;
        }

        if let Some(account) = labels.accounts.get(&parser_id) {
            plan.managed.push(ManagedLabel {
                label: label.clone(),
                parser_id,
                account: account.clone(),
            });
        } else if labels.is_ignored(&parser_id) {
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

    for parser_id in labels.accounts.keys() {
        if !seen.contains(parser_id) {
            plan.orphans.push(parser_id.clone());
        }
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> LabelsSection {
        LabelsSection {
            prefix: "receipt/".to_owned(),
            since: None,
            accounts: [
                ("example.card_debit".to_owned(), "uuid-example".to_owned()),
                ("wallet.qris_payment".to_owned(), "uuid-wallet".to_owned()),
            ]
            .into_iter()
            .collect(),
            ignore: vec!["merchant.promo".to_owned()],
        }
    }

    fn discovered(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    #[test]
    fn the_default_prefix_is_receipt_slash() {
        assert_eq!(LabelsSection::default().prefix, "receipt/");
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
        assert_eq!(plan.orphans, vec!["wallet.qris_payment"]);
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
                "receipt/wallet.qris_payment",
            ]),
            &labels(),
        );
        assert_eq!(plan.managed.len(), 2);
        assert!(plan.orphans.is_empty());
    }

    #[test]
    fn a_fully_mapped_inventory_has_nothing_to_do() {
        let plan = plan(
            &discovered(&["receipt/example.card_debit", "receipt/wallet.qris_payment"]),
            &labels(),
        );
        assert_eq!(plan.managed.len(), 2);
        assert!(plan.unmanaged.is_empty());
        assert!(plan.ignored.is_empty());
        assert!(plan.invalid.is_empty());
        assert!(plan.orphans.is_empty());
        assert!(!plan.needs_setup());
    }
}
