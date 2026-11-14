//! Turning `abmx-parser`'s parser corpus into a Gmail search expression.
//!
//! The corpus already knows every sender address a bank uses, so asking the
//! user to maintain a second list would be duplication that drifts. Wildcards in
//! a spec (`*@bank.example`) become a bare domain term, which Gmail treats as a
//! substring match on the address.

/// Build the `q=` expression used for a full sync.
///
/// `lookback_days` is a floor, not a filter: the state database deduplicates, so
/// a generous window costs one request and prevents silently missing mail after
/// a long outage. `0` disables the floor.
pub fn build_query(senders: &[String], lookback_days: u32, extra: Option<&str>) -> String {
    let mut terms: Vec<String> = Vec::new();

    let patterns: Vec<String> = senders
        .iter()
        .filter_map(|sender| normalize_sender(sender))
        .collect();

    if patterns.len() == 1 {
        terms.push(format!("from:{}", patterns[0]));
    } else if patterns.len() > 1 {
        terms.push(format!("from:({})", patterns.join(" OR ")));
    }

    terms.push("-in:spam".to_owned());
    terms.push("-in:trash".to_owned());

    if lookback_days > 0 {
        terms.push(format!("newer_than:{lookback_days}d"));
    }

    if let Some(extra) = extra {
        let extra = extra.trim();
        if !extra.is_empty() {
            terms.push(extra.to_owned());
        }
    }

    terms.join(" ")
}

fn normalize_sender(sender: &str) -> Option<String> {
    let sender = sender.trim();
    if sender.is_empty() {
        return None;
    }

    // `*@bank.example` and `alerts@*.bank.example` both reduce to the domain
    // fragment Gmail can match.
    if let Some((local, domain)) = sender.split_once('@') {
        if local == "*" || local.is_empty() {
            let domain = domain.trim_start_matches("*.").trim_matches('*');
            return (!domain.is_empty()).then(|| domain.to_owned());
        }
    }

    Some(sender.replace('*', ""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_sender_is_a_plain_from_term() {
        assert_eq!(
            build_query(&["alerts@example.invalid".into()], 7, None),
            "from:alerts@example.invalid -in:spam -in:trash newer_than:7d"
        );
    }

    #[test]
    fn multiple_senders_are_ored() {
        let query = build_query(&["a@example.invalid".into(), "c@wallet.example".into()], 0, None);
        assert_eq!(query, "from:(a@example.invalid OR c@wallet.example) -in:spam -in:trash");
    }

    #[test]
    fn wildcard_senders_become_domains() {
        let query = build_query(&["*@bank.example".into()], 0, None);
        assert_eq!(query, "from:bank.example -in:spam -in:trash");
    }

    #[test]
    fn extra_terms_are_appended() {
        let query = build_query(&[], 3, Some("-label:abmx-ignored"));
        assert_eq!(
            query,
            "-in:spam -in:trash newer_than:3d -label:abmx-ignored"
        );
    }

    #[test]
    fn duplicates_are_not_deduplicated_here() {
        // The registry may legitimately list the same sender twice; Gmail
        // tolerates it and dedup is not this function's job.
        let query = build_query(&["a@b.example".into(), "a@b.example".into()], 0, None);
        assert_eq!(query, "from:(a@b.example OR a@b.example) -in:spam -in:trash");
    }
}
