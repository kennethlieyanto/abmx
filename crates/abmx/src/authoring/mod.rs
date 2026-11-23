//! AI-assisted parser authoring.
//!
//! These are the pieces that turn `receipt/*` labels into download-ready samples,
//! normalized block dumps, a reusable skill, and a verification loop. Nothing
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
    pub const SPEC_REFERENCE: &str = include_str!("assets/spec-reference.md");
    pub const STATEMENT_REFERENCE: &str = include_str!("assets/statement-spec-reference.md");
    pub const AUTHORING_WORKFLOW: &str = include_str!("assets/authoring-workflow.md");
    pub const PITFALLS: &str = include_str!("assets/pitfalls.md");

    /// The host API version this guide was written against. Kept in the asset
    /// so a test can catch drift after an `abmx-parser` bump.
    #[allow(dead_code)]
    pub const SPEC_REFERENCE_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    #[test]
    fn spec_reference_tracks_parser() {
        assert_eq!(
            super::assets::SPEC_REFERENCE_VERSION,
            abmx_parser::API_VERSION
        );
        assert!(super::assets::SPEC_REFERENCE.contains("fn meta()"));
        assert!(super::assets::SPEC_REFERENCE.contains("parse_mail"));
    }
}
