//! PDF statement normalization.
//!
//! Input: encrypted or plain PDF bytes.
//! Output: pages of positioned text runs, the same shape regardless of the
//! producer. Nothing here knows about banks; `abmx-table` owns the grammar.

pub mod layout;
pub mod pdf;

pub use layout::{Document, Line, Page, Run};
pub use pdf::{load, Error};
