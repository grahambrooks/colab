//! Core types shared by every colab crate.
//!
//! This crate defines the [`CodeTransformer`] contract used by the
//! [`walker`], the language-backend plug-in surface
//! ([`LanguageBackend`], [`Operation`], [`BackendRegistry`]), and
//! crate-wide [`Error`]/[`Result`] types.

pub mod backend;
pub mod error;
pub mod template;
pub mod transformer;
pub mod walker;

pub use backend::{
    ActionCapability, BackendRegistry, Capability, LanguageBackend, Operation, RuleSpec,
};
pub use template::render_call_template;
pub use error::{Error, Result};
pub use transformer::CodeTransformer;
