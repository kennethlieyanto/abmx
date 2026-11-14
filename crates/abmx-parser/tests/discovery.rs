//! Parser discovery: precedence, merging, and the ConfigMap volume shape.

use std::fs;
use std::path::PathBuf;

use abmx_parser::{Options, Parser};

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("abmx-test-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn spec(id: &str, version: u32) -> String {
    format!(
        r#"spec_version = 1
id = "{id}"
version = {version}

[match]
senders  = ["alerts@example.com"]
subject  = '^Alert$'
requires = []
rejects  = []

[labels]
list = ["Amount", "Date"]

[defaults]
bank     = "test"
account  = "test-account"
currency = "IDR"
tz       = "Asia/Jakarta"
kind     = "purchase"

[extract.amount]
label  = "Amount"
regex  = '(?P<value>[0-9]+)'
number = "id"

[extract.occurred_at]
label   = "Date"
regex   = '(?P<value>\d{{2}}-\d{{2}}-\d{{4}})'
formats = ["%d-%m-%Y"]
locale  = "en"
"#
    )
}

#[test]
fn explicit_dirs_merge_and_first_wins() {
    let root = temp("merge");
    let base = root.join("base");
    let over = root.join("over");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&over).unwrap();

    fs::write(base.join("bank.one.toml"), spec("bank.one", 1)).unwrap();
    fs::write(over.join("bank.one.toml"), spec("bank.one", 9)).unwrap();
    fs::write(over.join("bank.two.toml"), spec("bank.two", 1)).unwrap();

    let options = Options {
        parsers: vec![base.clone(), over.clone()],
        router: None,
    };
    let (parser, discovery) = Parser::load(&options).expect("load");

    assert_eq!(discovery.dirs, vec![base, over]);
    assert_eq!(parser.registry().parsers.len(), 2);
    assert_eq!(
        parser.registry().find("bank.one").unwrap().spec.version,
        1,
        "the first directory must win"
    );
    assert_eq!(parser.registry().shadows.len(), 1);
    assert_eq!(parser.registry().shadows[0].id, "bank.one");
}

#[test]
fn duplicate_id_inside_one_directory_is_an_error() {
    let root = temp("duplicate");
    let nested = root.join("nested");
    fs::create_dir_all(&nested).unwrap();

    // Both files declare `bank.one`, so the id/file-stem check fires on the
    // second one before the duplicate check; either way loading must fail.
    fs::write(root.join("bank.one.toml"), spec("bank.one", 1)).unwrap();
    fs::write(nested.join("bank.one.toml"), spec("bank.one", 2)).unwrap();

    let result = Parser::load(&Options {
        parsers: vec![root],
        router: None,
    });
    assert!(result.is_err());
}

#[test]
fn id_must_match_the_file_name() {
    let root = temp("mismatch");
    fs::write(root.join("wrong.name.toml"), spec("bank.one", 1)).unwrap();

    let result = Parser::load(&Options {
        parsers: vec![root],
        router: None,
    });
    let error = result.err().expect("must fail").to_string();
    assert!(error.contains("does not match"), "got: {error}");
}

#[test]
fn missing_explicit_directory_is_an_error() {
    let root = temp("missing");
    let result = Parser::load(&Options {
        parsers: vec![root.join("nope")],
        router: None,
    });
    assert!(result.is_err());
}

#[test]
fn no_corpus_is_reported_as_such() {
    let root = temp("empty");
    let result = Parser::load(&Options {
        parsers: vec![root],
        router: None,
    });
    let error = result.err().expect("must fail").to_string();
    assert!(error.contains("no parser specs found"), "got: {error}");
}

#[cfg(unix)]
#[test]
fn configmap_style_symlinks_are_followed() {
    // A ConfigMap volume looks like: ..data/<key> with sibling symlinks.
    let root = temp("configmap");
    let data = root.join("..data");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("bank.one.toml"), spec("bank.one", 1)).unwrap();
    std::os::unix::fs::symlink(data.join("bank.one.toml"), root.join("bank.one.toml")).unwrap();

    let (parser, _) = Parser::load(&Options {
        parsers: vec![root],
        router: None,
    })
    .expect("load through symlinks");

    assert_eq!(parser.registry().parsers.len(), 1);
}
