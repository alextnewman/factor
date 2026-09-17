//! LLM backends. Provider-agnostic, OpenAI-protocol (§8.1).
//!
//! Ships two: a scripted mock (loop tests need no model) and the llama.cpp
//! HTTP backend (`/v1/chat/completions`, `cache_prompt: true`).

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::prompt::ChatMessage;
use crate::{FaError, Result};

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: Option<u64>,
    pub latency_ms: u64,
    /// Server-side prompt-eval time, when the server reports it (llama.cpp `timings`).
    pub server_prompt_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct ChatResponse {
    pub content: String,
    pub usage: Usage,
}

pub trait LlmBackend: Send + Sync {
    fn complete<'a>(
        &'a self,
        model: &'a str,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse>> + Send + 'a>>;
}

/// Scripted backend for tests and demos: pops one response per call.
pub struct MockBackend {
    script: Mutex<VecDeque<String>>,
}

impl MockBackend {
    pub fn new(responses: Vec<String>) -> Self {
        Self {
            script: Mutex::new(responses.into()),
        }
    }

    /// How many scripted responses remain.
    pub fn remaining(&self) -> usize {
        self.script.lock().unwrap().len()
    }
}

impl LlmBackend for MockBackend {
    fn complete<'a>(
        &'a self,
        _model: &'a str,
        _messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse>> + Send + 'a>> {
        Box::pin(async move {
            let content = self.script.lock().unwrap().pop_front().unwrap_or_default();
            Ok(ChatResponse {
                content,
                usage: Usage::default(),
            })
        })
    }
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    cache_prompt: bool,
    temperature: f32,
    max_tokens: u32,
    /// Qwen3-style reasoning models: keep tool-driving turns short and
    /// deterministic — the harness needs actions, not chain-of-thought.
    chat_template_kwargs: ChatTemplateKwargs,
}

#[derive(Serialize)]
struct ChatTemplateKwargs {
    enable_thinking: bool,
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    #[serde(default)]
    choices: Vec<Choice>,
    #[serde(default)]
    usage: Option<ResponseUsage>,
    #[serde(default)]
    timings: Option<Timings>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    message: Option<ResponseMessage>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct ResponseUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct Timings {
    #[serde(default)]
    prompt_ms: Option<f64>,
}

/// llama.cpp server backend. Platform TLS via reqwest/native-tls —
/// the Windows trust store (SChannel) is used on Windows.
pub struct LlamaCppBackend {
    base_url: String,
    client: reqwest::Client,
}

impl LlamaCppBackend {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| FaError::Backend(format!("http client: {e}")))?;
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client,
        })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl LlmBackend for LlamaCppBackend {
    fn complete<'a>(
        &'a self,
        model: &'a str,
        messages: &'a [ChatMessage],
    ) -> Pin<Box<dyn Future<Output = Result<ChatResponse>> + Send + 'a>> {
        Box::pin(async move {
            let t0 = Instant::now();
            let req = ChatCompletionRequest {
                model,
                messages: messages
                    .iter()
                    .map(|m| WireMessage {
                        role: &m.role,
                        content: &m.content,
                    })
                    .collect(),
                cache_prompt: true,
                temperature: 0.2,
                max_tokens: 1024,
                chat_template_kwargs: ChatTemplateKwargs {
                    enable_thinking: false,
                },
            };
            let url = format!("{}/v1/chat/completions", self.base_url);
            let http_resp = self
                .client
                .post(&url)
                .json(&req)
                .send()
                .await
                .map_err(|e| FaError::Backend(format!("llama.cpp request: {e}")))?;
            let status = http_resp.status();
            if !status.is_success() {
                let body = http_resp.text().await.unwrap_or_default();
                return Err(FaError::Backend(format!("llama.cpp HTTP {status}: {body}")));
            }
            let resp: ChatCompletionResponse = http_resp
                .json()
                .await
                .map_err(|e| FaError::Backend(format!("llama.cpp decode: {e}")))?;
            let content = resp
                .choices
                .first()
                .and_then(|c| c.message.as_ref())
                .and_then(|m| m.content.clone())
                .unwrap_or_default();
            let usage = resp
                .usage
                .map(|u| Usage {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    cached_tokens: u.prompt_tokens_details.and_then(|d| d.cached_tokens),
                    latency_ms: t0.elapsed().as_millis() as u64,
                    server_prompt_ms: resp
                        .timings
                        .as_ref()
                        .and_then(|t| t.prompt_ms)
                        .map(|ms| ms as u64),
                })
                .unwrap_or(Usage {
                    latency_ms: t0.elapsed().as_millis() as u64,
                    server_prompt_ms: resp
                        .timings
                        .as_ref()
                        .and_then(|t| t.prompt_ms)
                        .map(|ms| ms as u64),
                    ..Default::default()
                });
            Ok(ChatResponse { content, usage })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_pops_in_order() {
        let b = MockBackend::new(vec!["a".into(), "b".into()]);
        let r1 = b.complete("m", &[]).await.unwrap();
        let r2 = b.complete("m", &[]).await.unwrap();
        let r3 = b.complete("m", &[]).await.unwrap();
        assert_eq!(r1.content, "a");
        assert_eq!(r2.content, "b");
        assert_eq!(r3.content, "");
        assert_eq!(b.remaining(), 0);
    }
}
