//! End-to-end tests over the committed fixture corpus.
//!
//! Specs live in `parsers/<id>.toml`, fixtures in `fixtures/<id>/*.eml`. The
//! directory name is the parser id, so this is also a routing test: a fixture
//! under `fixtures/example.card_debit/` must be claimed by `example.card_debit`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use txp_parse::{Outcome, Registry};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

fn registry() -> Registry {
    Registry::load(
        repo_root().join("parsers"),
        repo_root().join("parsers/router.toml"),
    )
    .expect("load parsers")
}

/// `(parser id, fixture path)`, sorted.
fn fixtures() -> Vec<(String, PathBuf)> {
    let root = repo_root().join("fixtures");
    let mut files = Vec::new();
    collect(&root, &mut files);
    files.sort();

    files
        .into_iter()
        .map(|path| {
            let id = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string();
            (id, path)
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

fn parse(registry: &Registry, path: &Path) -> txp_core::Transaction {
    let bytes = std::fs::read(path).expect("read fixture");
    let email = txp_mail::parse_eml(&bytes, Some(path.display().to_string()))
        .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

    match registry.run(&email) {
        Outcome::Parsed(transaction) => *transaction,
        Outcome::Unmatched(reason) => panic!("{} was not routed: {reason}", path.display()),
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
        let transaction = parse(&registry, path);

        assert_eq!(&transaction.parser, expected_id, "{}", path.display());
        assert!(
            transaction.warnings.is_empty(),
            "{}: {:?}",
            path.display(),
            transaction.warnings
        );
        assert_ne!(transaction.amount.amount_minor, 0);
        assert_eq!(transaction.amount.currency, "IDR");

        seen.insert(transaction.parser.clone());

        // Two fixtures are byte-identical emails; they must collapse onto one
        // identity so a consumer's import stays idempotent.
        if let Some(message_id) = &transaction.source.message_id {
            if let Some(previous) = ids.insert(message_id.clone(), transaction.id.clone()) {
                assert_eq!(previous, transaction.id, "duplicates must share an id");
            }
        }
    }

    let expected: BTreeSet<String> = [
        "example.card_debit",
        "example.journal_qris",
        "example.journal_transfer",
        "example.journal_va",
        "wallet.card_online",
        "wallet.incoming_transfer",
        "wallet.online_order",
        "wallet.qris_payment",
        "banktwo.bank_transfer",
        "banktwo.qris_payment",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();

    assert_eq!(seen, expected);
}

#[test]
fn every_parser_has_fixtures() {
    let registry = registry();
    let ids: BTreeSet<String> = fixtures().into_iter().map(|(id, _)| id).collect();

    for parser in &registry.parsers {
        assert!(
            ids.contains(&parser.spec.id),
            "{} has no fixture directory under fixtures/{}",
            parser.spec.id,
            parser.spec.id
        );
    }
}

#[test]
fn amounts_dates_and_counterparties_are_correct() {
    let registry = registry();

    // (parser id, fixture file name, minor amount, local date, counterparty)
    let expectations: &[(&str, &str, i64, &str, &str)] = &[
        ("example.card_debit", "01.eml", -27_500, "2026-10-30", "ExamplePay"),
        ("example.card_debit", "02.eml", -11_000, "2026-11-03", "ExamplePay"),
        ("example.card_debit", "03.eml", -11_000, "2026-10-11", "ExamplePay"),
        (
            "example.journal_qris",
            "01.eml",
            -72_000,
            "2026-11-06",
            "EXAMPLE MERCHANT",
        ),
        (
            "example.journal_va",
            "01.eml",
            -550_000,
            "2026-11-02",
            "EXAMPLE MERCHANT",
        ),
        (
            "example.journal_transfer",
            "01.eml",
            -372_000,
            "2026-11-07",
            "EXAMPLE PERSON",
        ),
        (
            "wallet.incoming_transfer",
            "01.eml",
            100_000,
            "2025-10-22",
            "EXAMPLE MERCHANT",
        ),
        (
            "wallet.incoming_transfer",
            "02.eml",
            200_000,
            "2026-09-18",
            "EXAMPLE MERCHANT",
        ),
        (
            "wallet.qris_payment",
            "01.eml",
            -5_500,
            "2026-09-19",
            "EXAMPLE MERCHANT",
        ),
        (
            "wallet.card_online",
            "01.eml",
            -500,
            "2026-09-13",
            "DEI*AWS ID NON3DS",
        ),
        (
            "wallet.online_order",
            "01.eml",
            -19_000,
            "2026-09-23",
            "EXAMPLE MERCHANT",
        ),
        (
            "banktwo.qris_payment",
            "01.eml",
            -18_600,
            "2026-09-18",
            "EXAMPLE MERCHANT",
        ),
        (
            "banktwo.qris_payment",
            "02.eml",
            -4_269_000,
            "2026-07-12",
            "EXAMPLE MERCHANT",
        ),
        (
            "banktwo.qris_payment",
            "03.eml",
            -4_269_000,
            "2026-07-12",
            "EXAMPLE MERCHANT",
        ),
        (
            "banktwo.bank_transfer",
            "01.eml",
            -10_001_288,
            "2026-09-06",
            "EXAMPLE MERCHANT",
        ),
        (
            "banktwo.bank_transfer",
            "02.eml",
            -15_000_487,
            "2026-02-19",
            "EXAMPLE MERCHANT",
        ),
        (
            "banktwo.bank_transfer",
            "03.eml",
            -85_000,
            "2025-07-11",
            "EXAMPLE PERSON",
        ),
    ];

    assert_eq!(
        expectations.len(),
        fixtures().len(),
        "update the expectations when adding fixtures"
    );

    for (id, file, amount, date, counterparty) in expectations {
        let path = repo_root().join("fixtures").join(id).join(file);
        let transaction = parse(&registry, &path);

        assert_eq!(&transaction.parser, id, "{id}/{file}");
        assert_eq!(
            transaction.amount.amount_minor, *amount,
            "{id}/{file}: amount"
        );
        assert_eq!(transaction.currency_exponent, 0, "{id}/{file}: exponent");
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
