//! `txsync` — deliver parsed bank transactions to a ledger.
//!
//! The binary is a thin shell around four things:
//!
//! * [`config`] — `txsync.toml` plus environment overrides for secrets.
//! * [`state`] — a SQLite file holding cursors, delivered messages, and the
//!   review queue.
//! * [`runner`] — the pipeline: fetch, parse with `txp`, decide, plan, apply.
//! * [`fetchers`] — the sources that ship with the binary.
//!
//! Third-party sources and sinks implement `txsync_core::Fetcher` and
//! `txsync_core::Sink`; nothing here needs to change for that.

pub mod config;
pub mod fetchers;
pub mod paths;
pub mod runner;
pub mod state;

pub use config::Config;
pub use runner::Runner;
pub use state::State;
