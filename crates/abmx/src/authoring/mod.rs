//! AI-assisted parser authoring.
//!
//! These are the pieces that turn a directory of bank emails into an
//! inspectable task for an agent: normalized block dumps, a prompt, reusable
//! skills, and a verification loop. Nothing here calls a model or changes a
//! ledger; the output is text and files the user chooses to hand to an agent.

pub mod dump;
pub mod prompt;
pub mod samples;
pub mod skill;
pub mod verify;

pub use prompt::{render_prompt, IdFrom, PromptContext};
pub use samples::{Sample, SampleFailure, SampleGroup, SampleSet};
pub use verify::{VerifyOutcome, VerifyReport, VerifySample};

mod assets {
    pub const SPEC_REFERENCE: &str = include_str!("assets/spec-reference.md");
    pub const AUTHORING_WORKFLOW: &str = include_str!("assets/authoring-workflow.md");
    pub const PITFALLS: &str = include_str!("assets/pitfalls.md");

    /// The `spec_version` this guide was written against. Kept in the asset so
    /// a test can catch drift after an `abmx-parser` bump.
    pub const SPEC_REFERENCE_VERSION: u32 = 1;
}

#[cfg(test)]
mod tests {
    #[test]
    fn spec_reference_tracks_parser() {
        assert_eq!(
            super::assets::SPEC_REFERENCE_VERSION,
            abmx_parser::SPEC_VERSION
        );
        assert!(super::assets::SPEC_REFERENCE.contains("spec_version = 1"));
        assert!(super::assets::SPEC_REFERENCE.contains("[extract.amount]"));
    }
}
