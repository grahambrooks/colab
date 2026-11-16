//! Core types shared by every colab crate.
//!
//! This crate defines the [`CodeTransformer`] contract used by the
//! [`walker`], the language-backend plug-in surface
//! ([`LanguageBackend`], [`Operation`], [`BackendRegistry`]), and
//! crate-wide [`Error`]/[`Result`] types.

pub mod backend;
pub mod constraint;
pub mod error;
pub mod facts;
pub mod fences;
pub mod fileops;
pub mod find;
pub mod followup;
pub mod ignore;
pub mod merge;
pub mod near_miss;
pub mod prose;
pub mod render;
pub mod report;
pub mod run;
pub mod scoped;
pub mod script_file;
pub mod semantic;
pub mod suggest;
pub mod symbols;
pub mod template;
pub mod transformer;
pub mod walker;

pub use backend::{
    ALREADY, ActionCapability, AstInput, BackendRegistry, Capability, LanguageBackend, Operation,
    RuleSpec, SyntaxTree,
};
pub use constraint::{Constraint, Relation, Rewriter, Stop, Test, Transform, TransformArg, Var};
pub use error::{Error, ParseDetail, Result};
pub use fences::{Fence, FenceLocator, FencedOperation};
pub use followup::{Followup, WithFollowups};
pub use prose::{ProseLocator, ProseRename};
pub use render::{ChangeCollector, Detail, RenderOptions, write_unified_diff};
pub use report::{Conflict, RuleStat, RunReport, Severity, SkippedFile};
pub use scoped::ScopedOperation;
pub use template::render_call_template;
pub use transformer::{ApplyOutcome, CodeTransformer, RuleConflict};
pub use walker::{FileChange, WalkOptions, WalkOutcome};
