//! `abmx` — deliver parsed bank transactions to a ledger.
//!
//! The binary is a thin shell around four things:
//!
//! * [`config`] — `abmx.toml` plus environment overrides for secrets.
//! * [`state`] — a SQLite file holding cursors, delivered messages, and the
//!   review queue.
//! * [`runner`] — the pipeline: fetch, parse with `abmx-parser`, decide, plan, apply.
//! * [`fetchers`] — the sources that ship with the binary.
//! * [`authoring`] — block dumps, AI prompts, skills, and candidate-parser
//!   verification.
//!
//! Third-party sources and sinks implement `abmx_core::Fetcher` and
//! `abmx_core::Sink`; nothing here needs to change for that.

pub mod authoring;
pub mod config;
pub mod config_edit;
pub mod credentials;
pub mod fetchers;
pub mod gmail_auth;
pub mod labels;
pub mod paths;
pub mod runner;
pub mod setup;
pub mod state;

pub use config::Config;
pub use runner::Runner;
pub use state::State;
