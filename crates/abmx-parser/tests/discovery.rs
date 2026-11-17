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

fn spec(version: u32) -> String {
    format!(
        r#"spec_version = 2
version = {version}

[labels]
list = ["Amount", "Date"]

[defaults]
bank     = "test"
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

/// Write `root/<id>/config.toml`, the layout the loader expects.
fn write_parser(root: &std::path::Path, id: &str, version: u32) {
    let dir = root.join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), spec(version)).unwrap();
}

#[test]
fn explicit_dirs_merge_and_first_wins() {
    let root = temp("merge");
    let base = root.join("base");
    let over = root.join("over");
    fs::create_dir_all(&base).unwrap();
    fs::create_dir_all(&over).unwrap();

    write_parser(&base, "bank.one", 1);
    write_parser(&over, "bank.one", 9);
    write_parser(&over, "bank.two", 1);

    let options = Options {
        parsers: vec![base.clone(), over.clone()],
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
fn directory_name_is_the_parser_id() {
    let root = temp("id-from-dir");
    write_parser(&root, "bank.one", 1);

    let (parser, _) = Parser::load(&Options {
        parsers: vec![root],
    })
    .expect("load");

    assert_eq!(parser.registry().parsers[0].id, "bank.one");
    assert!(parser.registry().find("bank.one").is_some());
}

#[test]
fn directory_name_must_look_like_a_parser_id() {
    let root = temp("mismatch");
    let dir = root.join(".wrongname");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), spec(1)).unwrap();

    let result = Parser::load(&Options {
        parsers: vec![root],
    });
    let error = result.err().expect("must fail").to_string();
    assert!(error.contains("not a valid id"), "got: {error}");
}

#[test]
fn flat_layout_gets_an_actionable_error() {
    let root = temp("flat");
    fs::write(root.join("bank.one.toml"), spec(1)).unwrap();

    let error = Parser::load(&Options {
        parsers: vec![root],
    })
    .err()
    .expect("must fail")
    .to_string();

    assert!(error.contains("<id>/config.toml"), "got: {error}");
}

#[test]
fn missing_explicit_directory_is_an_error() {
    let root = temp("missing");
    let result = Parser::load(&Options {
        parsers: vec![root.join("nope")],
    });
    assert!(result.is_err());
}

#[test]
fn no_corpus_is_reported_as_such() {
    let root = temp("empty");
    let result = Parser::load(&Options {
        parsers: vec![root],
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
    fs::create_dir_all(data.join("bank.one")).unwrap();
    fs::write(data.join("bank.one/config.toml"), spec(1)).unwrap();
    std::os::unix::fs::symlink(data.join("bank.one"), root.join("bank.one")).unwrap();

    let (parser, _) = Parser::load(&Options {
        parsers: vec![root],
    })
    .expect("load through symlinks");

    assert_eq!(parser.registry().parsers.len(), 1);
}
