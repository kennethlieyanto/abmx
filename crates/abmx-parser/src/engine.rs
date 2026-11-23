//! The sandboxed Rhai engine and the compiled parser script.
//!
//! One engine is built per process and shared by the registry. Each parser is
//! compiled once into an [`AST`]; calls use a fresh [`Scope`] so a script cannot
//! leak state from one document to the next.
//!
//! The sandbox is deliberately small: no clock, no filesystem, no network, no
//! `eval`, no imports, and hard limits on operations, call depth, and allocation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rhai::packages::{
    ArithmeticPackage, BasicArrayPackage, BasicIteratorPackage, BasicMapPackage, BasicMathPackage,
    BasicStringPackage, LanguageCorePackage, LogicPackage, Package,
};
use rhai::{Dynamic, Engine, Position, Scope, AST};

use crate::spec::{InputKind, Meta};

/// The only file a parser directory may contain besides `fixtures/`.
pub const SCRIPT_FILE: &str = "parser.rhai";

/// Maximum script operations per document before the parser is aborted.
const MAX_OPERATIONS: u64 = 10_000_000;
/// Wall-clock ceiling per document. Protects against a pathological regex in a
/// host helper, which an operation counter cannot see.
const MAX_SECONDS: u64 = 10;

/// The engine and its per-call deadline.
///
/// The operation counter bounds a runaway loop; the deadline bounds a host
/// helper (a regex match) that an operation counter cannot see. The deadline is
/// refreshed before every call, so one slow document cannot poison the process.
pub struct Executor {
    engine: Engine,
    deadline: Arc<Mutex<Instant>>,
}

impl Default for Executor {
    fn default() -> Self {
        Self::new()
    }
}

impl Executor {
    pub fn new() -> Self {
        let deadline = Arc::new(Mutex::new(Instant::now()));
        let mut engine = Engine::new_raw();

        // The language core plus the collections a table parser needs. Notably not
        // `StandardPackage`: no clock, no blobs, no debug helpers.
        LanguageCorePackage::new().register_into_engine(&mut engine);
        ArithmeticPackage::new().register_into_engine(&mut engine);
        LogicPackage::new().register_into_engine(&mut engine);
        BasicMathPackage::new().register_into_engine(&mut engine);
        BasicStringPackage::new().register_into_engine(&mut engine);
        BasicArrayPackage::new().register_into_engine(&mut engine);
        BasicMapPackage::new().register_into_engine(&mut engine);
        BasicIteratorPackage::new().register_into_engine(&mut engine);

        engine.set_max_operations(MAX_OPERATIONS);
        engine.set_max_call_levels(64);
        engine.set_max_expr_depths(64, 64);
        engine.set_max_string_size(256 * 1024);
        engine.set_max_array_size(100_000);
        engine.set_max_map_size(10_000);

        // `print` exists as a string helper; keep it out of stdout. A parser that
        // needs to explain itself should return a warning instead.
        engine.on_print(|_| {});
        engine.on_debug(|_, _, _| {});

        let watch = Arc::clone(&deadline);
        engine.on_progress(move |_| {
            let expired = watch
                .lock()
                .map(|deadline| Instant::now() > *deadline)
                .unwrap_or(true);
            expired.then(|| Dynamic::from("parser exceeded its time limit"))
        });

        crate::mail::register(&mut engine);
        crate::statement::register(&mut engine);

        Self { engine, deadline }
    }

    fn arm(&self) {
        if let Ok(mut deadline) = self.deadline.lock() {
            *deadline = Instant::now() + Duration::from_secs(MAX_SECONDS);
        }
    }

    /// Call a script function with a fresh scope and a fresh deadline.
    pub fn call(
        &self,
        ast: &AST,
        name: &str,
        args: impl rhai::FuncArgs,
    ) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        self.arm();
        let mut scope = Scope::new();
        self.engine.call_fn(&mut scope, ast, name, args)
    }

    /// Evaluate a snippet in the host engine. Only for tests and tooling;
    /// scripts cannot reach `eval`.
    pub fn eval(&self, source: &str) -> Result<Dynamic, Box<rhai::EvalAltResult>> {
        self.arm();
        self.engine.eval(source)
    }

    pub fn compile(&self, path: &Path, source: &str) -> Result<AST, String> {
        self.engine
            .compile(source)
            .map_err(|error| describe_parse_error(path, &error))
    }
}

/// Build the engine every parser runs in.
pub fn engine() -> Executor {
    Executor::new()
}

/// A compiled `parser.rhai`.
#[derive(Debug)]
pub struct Script {
    pub id: String,
    pub meta: Meta,
    pub source_file: PathBuf,
    pub(crate) ast: AST,
}

impl Script {
    /// Compile a script and read its `meta()`.
    pub fn load(executor: &Executor, path: &Path, id: String) -> Result<Self, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        // Checked before compiling: a constant `eval("...")` is folded by the
        // optimizer, so it never reaches the AST.
        reject_eval_source(&source)?;
        let ast = executor.compile(path, &source)?;
        reject_unsafe_calls(&ast)?;

        let functions: Vec<String> = ast
            .iter_functions()
            .filter(|function| function.access == rhai::FnAccess::Public)
            .map(|function| function.name.to_owned())
            .collect();

        let mail = functions.iter().any(|name| name == "parse_mail");
        let statement = functions.iter().any(|name| name == "parse_statement");
        let input = match (mail, statement) {
            (true, false) => InputKind::Mail,
            (false, true) => InputKind::Statement,
            (false, false) => {
                return Err(
                    "a parser must define `fn parse_mail(ctx)` or `fn parse_statement(doc, ctx)`"
                        .to_owned(),
                )
            }
            (true, true) => {
                return Err(
                    "a parser may define `parse_mail` or `parse_statement`, not both".to_owned(),
                )
            }
        };

        if !functions.iter().any(|name| name == "meta") {
            return Err("every parser must define `fn meta()`".to_owned());
        }

        let value: Dynamic = executor
            .call(&ast, "meta", ())
            .map_err(|error| describe_runtime_error("meta()", &error))?;
        let meta = Meta::from_dynamic(&value, input, &id)?;

        Ok(Self {
            id,
            meta,
            source_file: path.to_path_buf(),
            ast,
        })
    }

    pub fn input(&self) -> InputKind {
        self.meta.input
    }

    pub fn summary(&self) -> crate::spec::SpecSummary {
        crate::spec::SpecSummary {
            id: self.id.clone(),
            version: self.meta.version,
            bank: self.meta.bank.clone(),
            kind: self.meta.kind,
            input: self.meta.input.as_str(),
            file: self.source_file.clone(),
        }
    }
}

/// The directory a parser's fixtures live in.
pub fn fixtures_dir(script: &Script) -> PathBuf {
    script
        .source_file
        .parent()
        .map(|dir| dir.join("fixtures"))
        .unwrap_or_default()
}

/// `disable_symbol` does not cover the built-in `eval(...)` call form, so the
/// AST is checked at load time. Nothing else in the host API evaluates strings.
fn reject_eval_source(source: &str) -> Result<(), String> {
    // `eval` is not a helper name in this corpus, and a false positive in a
    // comment or string is cheap to rename. This is the only reliable place to
    // catch the constant form, which the optimizer evaluates at compile time.
    let uses_eval = source
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .any(|token| token == "eval");

    if uses_eval {
        return Err(
            "`eval` is not available to parsers; return data instead of evaluating strings"
                .to_owned(),
        );
    }
    Ok(())
}

fn reject_unsafe_calls(ast: &AST) -> Result<(), String> {
    let mut banned = None;
    ast.walk(&mut |nodes| {
        for node in nodes {
            if let rhai::ASTNode::Expr(rhai::Expr::FnCall(call, _)) = node {
                if call.name.as_str() == "eval" {
                    banned = Some(call.name.to_string());
                    return false;
                }
            }
        }
        true
    });

    match banned {
        Some(name) => Err(format!(
            "`{name}` is not available to parsers; return data instead of evaluating strings"
        )),
        None => Ok(()),
    }
}

fn describe_parse_error(path: &Path, error: &rhai::ParseError) -> String {
    format!("{}: {}", path.display(), error)
}

pub(crate) fn describe_runtime_error(entry: &str, error: &rhai::EvalAltResult) -> String {
    let position = match error.position() {
        Position::NONE => String::new(),
        position => format!(" at {position}"),
    };

    match error {
        rhai::EvalAltResult::ErrorFunctionNotFound(name, _) => format!(
            "{entry}: `{name}` is not a host function{position}; see `abmx skill receipt` or `abmx skill statement` for the API"
        ),
        rhai::EvalAltResult::ErrorTooManyOperations(_) => {
            format!("{entry}: parser exceeded its operation limit{position}")
        }
        rhai::EvalAltResult::ErrorTerminated(..) => {
            format!("{entry}: parser exceeded its time limit{position}")
        }
        other => format!("{entry}: {other}{position}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_exposes_no_io() {
        let executor = Executor::new();
        for script in [
            r#"read_file("/etc/passwd")"#,
            r#"system("ls")"#,
            r#"import "other" as other;"#,
        ] {
            assert!(executor.eval(script).is_err(), "script must fail: {script}");
        }
    }

    #[test]
    fn eval_is_rejected_at_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("parser.rhai");
        std::fs::write(
            &path,
            r#"
fn meta() { #{ bank: "test", kind: "purchase", labels: ["A"] } }
fn parse_mail(ctx) { let script = "1"; eval(script) }
"#,
        )
        .unwrap();

        let error = Script::load(&Executor::new(), &path, "test".to_owned()).unwrap_err();
        assert!(error.contains("eval"), "{error}");
    }

    #[test]
    fn runaway_loops_are_stopped() {
        let executor = Executor::new();
        let error = executor.eval("let x = 0; loop { x += 1; }").unwrap_err();
        assert!(
            matches!(*error, rhai::EvalAltResult::ErrorTooManyOperations(_)),
            "{error}"
        );
    }
}
