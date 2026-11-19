//! End-to-end tests over the committed synthetic example corpus.
//!
//! A parser is a directory: `examples/parsers/<id>/config.toml` with its samples
//! in `examples/parsers/<id>/fixtures/*.eml`. The directory name is the parser
//! id, so this is also a routing test.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use abmx_parse::{Outcome, Registry};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn registry() -> Registry {
    Registry::load(repo_root().join("examples/parsers")).expect("load example parsers")
}

/// `(parser id, fixture path)`, sorted.
fn fixtures() -> Vec<(String, PathBuf)> {
    let root = repo_root().join("examples/parsers");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();

    files
        .into_iter()
        .filter_map(|path| {
            // examples/parsers/<id>/fixtures/<file>.eml
            let id = path
                .parent()
                .and_then(|fixtures| fixtures.parent())
                .and_then(|parser| parser.file_name())
                .and_then(|name| name.to_str())
                .map(str::to_string)?;
            id.contains('.').then_some((id, path))
        })
        .collect()
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|value| value.to_str()) == Some("eml") {
            out.push(path);
        }
    }
}

fn parse(registry: &Registry, id: &str, path: &Path) -> abmx_tx::Transaction {
    let bytes = std::fs::read(path).expect("read fixture");
    let email = abmx_mail::parse_eml(&bytes, Some(path.display().to_string()))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    match registry.run(id, &email, "test") {
        Outcome::Parsed(transaction) => *transaction,
        Outcome::Missing(parser) => panic!("{} names missing parser {parser}", path.display()),
        Outcome::Failed { parser, error } => {
            panic!("{} failed in {parser}: {error}", path.display())
        }
    }
}

#[test]
fn every_fixture_is_parsed() {
    let registry = registry();
    let fixtures = fixtures();
    assert!(!fixtures.is_empty(), "no fixtures found");

    let mut seen = BTreeSet::new();
    let mut ids = std::collections::BTreeMap::new();

    for (expected_id, path) in &fixtures {
        let transaction = parse(&registry, expected_id, path);

        assert_eq!(&transaction.parser, expected_id, "{}", path.display());
        assert!(
            transaction.warnings.is_empty(),
            "{}: {:?}",
            path.display(),
            transaction.warnings
        );
        assert_ne!(transaction.amount.amount_minor, 0);
        assert_eq!(transaction.amount.currency, "USD");

        seen.insert(transaction.parser.clone());

        if let Some(message_id) = &transaction.source.message_id {
            if let Some(previous) = ids.insert(message_id.clone(), transaction.id.clone()) {
                assert_eq!(previous, transaction.id, "duplicates must share an id");
            }
        }
    }

    let expected: BTreeSet<String> = ["example.bank"].into_iter().map(str::to_string).collect();

    assert_eq!(seen, expected);
}

#[test]
fn every_parser_has_fixtures() {
    let registry = registry();
    let ids: BTreeSet<String> = fixtures().into_iter().map(|(id, _)| id).collect();

    for parser in &registry.parsers {
        assert!(
            ids.contains(&parser.id),
            "{} has no fixtures under examples/parsers/{}/fixtures/",
            parser.id,
            parser.id
        );
    }
}

#[test]
fn amounts_dates_and_counterparties_are_correct() {
    let registry = registry();

    // (parser id, fixture file name, minor amount, local date, counterparty)
    let expectations: &[(&str, &str, i64, &str, &str)] = &[(
        "example.bank",
        "01.eml",
        -2_750,
        "2026-10-30",
        "Example Merchant",
    )];

    assert_eq!(
        expectations.len(),
        fixtures().len(),
        "update the expectations when adding fixtures"
    );

    for (id, file, amount, date, counterparty) in expectations {
        let path = repo_root()
            .join("examples/parsers")
            .join(id)
            .join("fixtures")
            .join(file);
        let transaction = parse(&registry, id, &path);

        assert_eq!(&transaction.parser, id, "{id}/{file}");
        assert_eq!(
            transaction.amount.amount_minor, *amount,
            "{id}/{file}: amount"
        );
        assert_eq!(transaction.currency_exponent, 2, "{id}/{file}: exponent");
        assert_eq!(
            transaction.occurred_on.to_string(),
            *date,
            "{id}/{file}: date"
        );
        assert_eq!(
            transaction.counterparty.as_deref(),
            Some(*counterparty),
            "{id}/{file}: counterparty"
        );
    }
}

#[test]
fn a_transaction_date_far_from_the_email_date_is_still_strict() {
    let registry = registry();
    let path = repo_root().join("examples/parsers/example.bank/fixtures/01.eml");
    let bytes = std::fs::read(&path).expect("read fixture");
    let mut email = abmx_mail::parse_eml(&bytes, Some(path.display().to_string())).expect("parse");

    // Send late and batched alerts: the gap between the transaction date and
    // the email date is data, not an extraction failure.
    email.date -= chrono::Duration::days(30);

    match registry.run("example.bank", &email, "test") {
        Outcome::Parsed(transaction) => {
            assert_eq!(transaction.extraction, abmx_tx::Extraction::Strict);
            assert!(
                transaction.warnings.is_empty(),
                "{:?}",
                transaction.warnings
            );
        }
        other => panic!("expected a transaction, got {other:?}"),
    }
}
