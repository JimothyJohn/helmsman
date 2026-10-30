//! The agent behind the box's chat panel (card `agent-loop`).
//!
//! Claude is reached only through the relay ([`relay`]), with a per-box
//! token; the box holds no Anthropic key and names no model, prompt or
//! tool schema (the relay serves those per agent version). The loop
//! ([`agent`]) streams each assistant turn ([`sse`], [`turn`]), runs the
//! tool calls on the box against its own HTTP API ([`tools`]), and sends
//! the results back until the model ends its turn or a budget runs out.
//!
//! The safety story is not in a prompt. The tools are a fixed set that
//! can read, draft, check, simulate and propose; none can apply. Apply is
//! the approval policy's, behind a PIN, on a route no tool reaches. Plant
//! text (tag names, ST comments, manifests, bundle text) reaches the model
//! only inside a marked data envelope ([`tools::envelope`]).

#![forbid(unsafe_code)]

pub mod agent;
pub mod http;
pub mod relay;
pub mod sse;
pub mod tools;
pub mod turn;

pub use agent::{Agent, Budget, Conversation, Limits, Outcome, Progress, Retry, Stop};
pub use tools::BoxApi;
