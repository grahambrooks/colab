//! Core types shared by every colab crate.
//!
//! This crate defines the [`CodeTransformer`] contract used by the
//! [`walker`], the language-backend plug-in surface
//! ([`LanguageBackend`], [`Operation`], [`BackendRegistry`]), and
//! crate-wide [`Error`]/[`Result`] types.

pub mod backend;
pub mod constraint;
pub mod error;
pub mod fileops;
pub mod find;
pub mod followup;
pub mod near_miss;
pub mod render;
pub mod report;
pub mod scoped;
pub mod suggest;
pub mod template;
pub mod transformer;
pub mod walker;

pub use backend::{
    ActionCapability, AstInput, BackendRegistry, Capability, LanguageBackend, Operation, RuleSpec,
    SyntaxTree,
};
pub use constraint::{Constraint, Test, Var};
pub use error::{Error, ParseDetail, Result};
pub use followup::{Followup, WithFollowups};
pub use render::{ChangeCollector, Detail, RenderOptions, write_unified_diff};
pub use report::{RuleStat, RunReport, SkippedFile};
pub use scoped::ScopedOperation;
pub use template::render_call_template;
pub use transformer::{ApplyOutcome, CodeTransformer};
pub use walker::{FileChange, WalkOptions, WalkOutcome};
