//! fa-core: the portable agent engine library.
//!
//! ACP types, the agent loop, the approval model, the execution-backend
//! trait, LLM backends (mock + llama.cpp), prompt construction, scopes,
//! and the session store. No tool logic lives here — tools are PowerShell.

pub mod agent;
pub mod approver;
pub mod backend;
pub mod dialect;
pub mod error;
pub mod executor;
pub mod manifest;
pub mod prompt;
pub mod protocol;
pub mod room;
pub mod rpc;
pub mod session;

pub use error::{FaError, Result};
