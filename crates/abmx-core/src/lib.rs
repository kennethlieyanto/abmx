//! Transport-neutral primitives for `abmx`.
//!
//! Nothing in this crate performs I/O. It defines the four seams that every
//! integration plugs into:
//!
//! * [`Fetcher`] — a source of raw RFC 5322 bytes (Gmail, IMAP, a directory).
//! * [`Sink`] — a destination for decided transactions (Actual, a file, a webhook).
//! * [`policy`] — the pure function that turns a [`abmx_parser::Transaction`] into
//!   "import" or a named defect.
//! * [`Plan`] — the inspectable artifact between parsing and writing.
//!
//! The split matters because the two hard problems are different: `abmx-parser` owns
//! *parsing*, `abmx` owns *delivery*. A new bank is a TOML file in `abmx-parser`; a new
//! mailbox or ledger is an implementation of one trait here.

pub mod diff;
pub mod error;
pub mod fetcher;
pub mod money;
pub mod plan;
pub mod policy;
pub mod sink;

pub use diff::{classify, LedgerMatch, LedgerRow};
pub use error::{Error, Result};
pub use fetcher::{FetchBatch, FetchRequest, Fetcher, MessageKey, RawMessage};
pub use money::cents;
pub use plan::{
    BrokenMessage, Plan, PlannedRow, SourceReport, Stats, UnmatchedMessage, PLAN_VERSION,
};
pub use policy::{check_transaction, AccountMap, BrokenReason};
pub use sink::{ApplyOptions, ImportRow, Report, RowError, Sink, SinkAccount};

/// The `abmx-parser` output contract this build was compiled against.
///
/// Persisted in the state database so a parser-contract bump cannot silently
/// reinterpret rows written by an older build.
pub use abmx_parser::SCHEMA_VERSION as ABMX_SCHEMA_VERSION;

/// The transaction contract itself.
///
/// Taken from `abmx-tx` because `abmx-parser` uses the type internally without
/// re-exporting it. When `abmx-parser` re-exports `Transaction`, this alias is the only
/// line that changes.
pub use abmx_tx::Transaction;
