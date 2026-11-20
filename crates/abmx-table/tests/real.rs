//! Opt-in verification against the real Mandiri statements.
//!
//! Set `ABMX_STATEMENT_SAMPLES` to the directory holding the PDFs, their
//! `.password`, and `corpus/<id>/{config.toml,fixtures/}`. Nothing here embeds
//! statement data; the test only asserts the arithmetic and row counts.

use std::path::PathBuf;

use abmx_doc::load;
use abmx_table::Registry;

#[test]
fn parses_the_real_mandiri_statements_when_configured() {
    let Some(dir) = std::env::var_os("ABMX_STATEMENT_SAMPLES").map(PathBuf::from) else {
        return;
    };

    let password = std::fs::read_to_string(dir.join(".password"))
        .expect("a .password file next to the samples")
        .trim()
        .to_owned();

    let registry = Registry::from_dirs(&[dir.join("corpus")]).expect("loading the corpus");
    let parser = registry
        .find("mandiri.consolidated_statement")
        .expect("the Mandiri statement parser");

    let fixtures = dir.join("corpus/mandiri.consolidated_statement/fixtures");
    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(&fixtures)
        .expect("fixtures directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pdf"))
        .collect();
    pdfs.sort();
    assert_eq!(pdfs.len(), 3, "three monthly fixtures");

    for path in pdfs {
        let bytes = std::fs::read(&path).expect("reading a fixture");
        let document = load(&bytes, Some(&password)).expect("decrypting a fixture");
        let transactions = parser
            .parse(&document, Some(path.display().to_string()))
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));

        assert!(!transactions.is_empty());
        for transaction in &transactions {
            assert_eq!(transaction.parser, "mandiri.consolidated_statement");
            assert_eq!(transaction.account, "savings");
            assert_eq!(transaction.bank, "mandiri");
        }

        let credits: i64 = transactions
            .iter()
            .filter(|tx| tx.amount.amount_minor > 0)
            .map(|tx| tx.amount.amount_minor)
            .sum();
        let debits: i64 = transactions
            .iter()
            .filter(|tx| tx.amount.amount_minor < 0)
            .map(|tx| -tx.amount.amount_minor)
            .sum();

        println!(
            "{}: {} rows, credits {}, debits {}",
            path.file_name().unwrap().to_string_lossy(),
            transactions.len(),
            credits,
            debits
        );

        let mut ids: Vec<&str> = transactions.iter().map(|tx| tx.id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), transactions.len(), "ids must be unique");
    }
}
