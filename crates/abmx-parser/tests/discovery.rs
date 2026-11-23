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

fn script(version: u32) -> String {
    format!(
        r#"
fn meta() {{
    #{{
        bank: "test",
        currency: "IDR",
        tz: "Asia/Jakarta",
        offset: "+07:00",
        kind: "purchase",
        labels: ["Amount", "Date"],
        api: 1,
        version: {version},
    }}
}}

fn parse_mail(ctx) {{
    #{{
        amount: ctx.money_in("Amount", "id"),
        occurred_at: ctx.date_in("Date", ["%d-%m-%Y"], "en"),
        counterparty: "test",
    }}
}}
"#
    )
}

/// Write `root/<id>/parser.rhai`, the layout the loader expects.
fn write_parser(root: &std::path::Path, id: &str, version: u32) {
    let dir = root.join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("parser.rhai"), script(version)).unwrap();
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
        parser.registry().find("bank.one").unwrap().meta.version,
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
    fs::write(dir.join("parser.rhai"), script(1)).unwrap();

    let result = Parser::load(&Options {
        parsers: vec![root],
    });
    let error = result.err().expect("must fail").to_string();
    assert!(error.contains("not a valid id"), "got: {error}");
}

#[test]
fn old_toml_layout_gets_an_actionable_error() {
    let root = temp("old-layout");
    let dir = root.join("bank.one");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("config.toml"), "spec_version = 2\n").unwrap();

    let error = Parser::load(&Options {
        parsers: vec![root],
    })
    .err()
    .expect("must fail")
    .to_string();

    assert!(error.contains("parser.rhai"), "got: {error}");
}

#[test]
fn missing_entry_point_is_rejected() {
    let root = temp("no-entry");
    let dir = root.join("bank.one");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("parser.rhai"),
        r#"fn meta() { #{ bank: "test", kind: "purchase", labels: ["A"] } }"#,
    )
    .unwrap();

    let error = Parser::load(&Options {
        parsers: vec![root],
    })
    .err()
    .expect("must fail")
    .to_string();

    assert!(error.contains("parse_mail"), "got: {error}");
}

#[test]
fn both_entry_points_are_rejected() {
    let root = temp("both-entry");
    let dir = root.join("bank.one");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("parser.rhai"),
        r#"
fn meta() { #{ bank: "test", kind: "purchase", labels: ["A"] } }
fn parse_mail(ctx) { #{} }
fn parse_statement(doc) { #{} }
"#,
    )
    .unwrap();

    let error = Parser::load(&Options {
        parsers: vec![root],
    })
    .err()
    .expect("must fail")
    .to_string();

    assert!(error.contains("not both"), "got: {error}");
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
    assert!(error.contains("no parser scripts found"), "got: {error}");
}

#[cfg(unix)]
#[test]
fn configmap_style_symlinks_are_followed() {
    // A ConfigMap volume looks like: ..data/<key> with sibling symlinks.
    let root = temp("configmap");
    let data = root.join("..data");
    fs::create_dir_all(data.join("bank.one")).unwrap();
    fs::write(data.join("bank.one/parser.rhai"), script(1)).unwrap();
    std::os::unix::fs::symlink(data.join("bank.one"), root.join("bank.one")).unwrap();

    let (parser, _) = Parser::load(&Options {
        parsers: vec![root],
    })
    .expect("load through symlinks");

    assert_eq!(parser.registry().parsers.len(), 1);
}
