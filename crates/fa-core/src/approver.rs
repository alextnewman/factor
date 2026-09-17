//! Approval model: per-chain approval with WhatIf expansion (§4.5).
//!
//! A human thinks "find → patch → rebuild" as one intention; the policy
//! sees what the human sees. The approver is a trait so tests script it
//! and the console (or a socket peer) implements it for real.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::protocol::ToolCall;
use crate::Result;

/// One approval unit: the whole chain the model proposed in a turn.
#[derive(Debug, Clone)]
pub struct ApprovalRequest {
    pub chain: Vec<ToolCall>,
    pub previews: Vec<String>,
}

#[derive(Debug, Clone)]
pub enum ApprovalDecision {
    Approve,
    Deny,
    /// Replace the args of the single call in the chain (only meaningful
    /// when the chain has exactly one call).
    Edit(Map<String, Value>),
}

pub trait Approver: Send + Sync {
    fn decide<'a>(
        &'a self,
        req: &'a ApprovalRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ApprovalDecision>> + Send + 'a>>;
}

/// Test/scripted approver: always approves.
pub struct AutoApprover;

impl Approver for AutoApprover {
    fn decide<'a>(
        &'a self,
        _req: &'a ApprovalRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ApprovalDecision>> + Send + 'a>> {
        Box::pin(async move { Ok(ApprovalDecision::Approve) })
    }
}

/// Scripted denier, for testing the deny path.
pub struct DenyApprover;

impl Approver for DenyApprover {
    fn decide<'a>(
        &'a self,
        _req: &'a ApprovalRequest,
    ) -> Pin<Box<dyn Future<Output = Result<ApprovalDecision>> + Send + 'a>> {
        Box::pin(async move { Ok(ApprovalDecision::Deny) })
    }
}

pub type ApproverRef = Arc<dyn Approver>;
