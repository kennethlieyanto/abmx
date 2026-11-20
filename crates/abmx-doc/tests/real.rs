//! Opt-in verification against real statements.
//!
//! Real bank statements are PII and are never committed. Set
//! `ABMX_STATEMENT_SAMPLES` to a directory containing the PDFs and a
//! `.password` file to run this test locally. `ABMX_STATEMENT_DUMP=1` prints
//! the positioned lines of the last page, which is how the column ranges in a
//! parser spec are chosen.

use std::path::PathBuf;

use abmx_doc::load;

#[test]
fn extracts_real_statement_samples_when_configured() {
    let Some(dir) = std::env::var_os("ABMX_STATEMENT_SAMPLES").map(PathBuf::from) else {
        return;
    };

    let password = std::fs::read_to_string(dir.join(".password"))
        .expect("a .password file next to the samples")
        .trim()
        .to_owned();

    let mut pdfs: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("samples directory")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pdf"))
        .collect();
    pdfs.sort();
    assert!(!pdfs.is_empty(), "no PDFs in {}", dir.display());

    let dump = std::env::var_os("ABMX_STATEMENT_DUMP").is_some();

    for path in pdfs {
        let bytes = std::fs::read(&path).expect("reading a sample");
        let document = load(&bytes, Some(&password)).expect("decrypting a sample");
        assert!(
            document.pages.len() >= 3,
            "{}: expected at least 3 pages",
            path.display()
        );
        for page in &document.pages {
            assert!(!page.lines.is_empty(), "{}: empty page", path.display());
        }
        println!(
            "{}: {} pages, {} lines on the last page",
            path.display(),
            document.pages.len(),
            document
                .pages
                .last()
                .map(|page| page.lines.len())
                .unwrap_or(0)
        );

        if dump {
            let page = document.pages.last().expect("a page");
            for line in &page.lines {
                print!("{:8.2} |", line.y);
                for run in &line.runs {
                    print!(" [{:6.1}] {:?}", run.x, run.text);
                }
                println!();
            }
        }
    }
}

#[test]
fn a_wrong_password_is_reported_as_such() {
    let Some(dir) = std::env::var_os("ABMX_STATEMENT_SAMPLES").map(PathBuf::from) else {
        return;
    };
    let path = std::fs::read_dir(&dir)
        .expect("samples directory")
        .flatten()
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pdf"))
        .expect("a sample PDF");
    let bytes = std::fs::read(path).expect("reading a sample");

    assert!(matches!(
        load(&bytes, Some("definitely-not-the-password")),
        Err(abmx_doc::Error::WrongPassword)
    ));
    assert!(matches!(load(&bytes, None), Err(abmx_doc::Error::Locked)));
}
