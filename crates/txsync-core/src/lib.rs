//! Transport-neutral primitives for `txsync`.
//!
//! Nothing in this crate performs I/O. It defines the four seams that every
//! integration plugs into:
//!
//! * [`Fetcher`] — a source of raw RFC 5322 bytes (Gmail, IMAP, a directory).
//! * [`Sink`] — a destination for decided transactions (Actual, a file, a webhook).
//! * [`policy`] — the pure function that turns a [`txp::Transaction`] into a
//!   [`Decision`].
//! * [`Plan`] — the inspectable artifact between deciding and writing.
//!
//! The split matters because the two hard problems are different: `txp` owns
//! *parsing*, `txsync` owns *delivery*. A new bank is a TOML file in `txp`; a new
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
pub use fetcher::{FetchBatch, FetchRequest, Fetcher, MessageKey, RawMessage, DEFAULT_LIMIT};
pub use money::cents;
pub use plan::{Plan, PlannedRow, SourceReport, Stats, PLAN_VERSION};
pub use policy::{
    decide, AccountMap, Decision, FailedPolicy, HeuristicPolicy, PolicyConfig, ReviewReason,
    SkipReason,
};
pub use sink::{ApplyOptions, ImportRow, Report, RowError, Sink, SinkAccount};

/// The `txp` output contract this build was compiled against.
///
/// Persisted in the state database so a parser-contract bump cannot silently
/// reinterpret rows written by an older build.
pub use txp::SCHEMA_VERSION as TXP_SCHEMA_VERSION;

/// The transaction contract itself.
///
/// Taken from `txp-core` because `txp` uses the type internally without
/// re-exporting it. When `txp` re-exports `Transaction`, this alias is the only
/// line that changes.
pub use txp_core::Transaction;
