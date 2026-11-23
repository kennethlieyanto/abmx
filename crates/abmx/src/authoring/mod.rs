//! AI-assisted parser authoring.
//!
//! These are the pieces that turn `receipt/*` labels into download-ready samples,
//! normalized block dumps, reusable skills, and a verification loop. Nothing
//! here calls a model or changes a ledger; the output is text and files the user
//! chooses to hand to an agent.

pub mod author;
pub mod dump;
pub mod samples;
pub mod skill;
pub mod verify;

pub use author::{Inventory, LabelReport, LabelStatus};
pub use samples::{Sample, SampleFailure, SampleGroup, SampleSet};
pub use verify::{VerifyOutcome, VerifyReport, VerifySample};

mod assets {
    pub const SKILL_INDEX: &str = include_str!("assets/skill.md");
    pub const RECEIPT_REFERENCE: &str = include_str!("assets/author-receipt-parser.md");
    pub const STATEMENT_REFERENCE: &str = include_str!("assets/author-statement-parser.md");

    /// The host API version these guides were written against. Kept here so a
    /// test can catch drift after an `abmx-parser` bump.
    #[cfg(test)]
    pub const SKILL_API_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    #[test]
    fn skills_track_parser() {
        assert_eq!(super::assets::SKILL_API_VERSION, abmx_parser::API_VERSION);
        assert!(super::assets::SKILL_INDEX.contains("author-receipt-parser.md"));
        assert!(super::assets::SKILL_INDEX.contains("author-statement-parser.md"));
        assert!(super::assets::RECEIPT_REFERENCE.contains("fn parse_mail(ctx)"));
        assert!(super::assets::STATEMENT_REFERENCE.contains("fn parse_statement(doc)"));
    }
}
