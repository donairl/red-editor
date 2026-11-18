//! The Agent Client Protocol (<https://agentclientprotocol.com>): Red is the
//! client, a coding agent is a subprocess that speaks JSON-RPC 2.0 over its
//! stdin and stdout, one message per line.

pub mod client;
#[cfg(test)]
mod fake_agent;
pub mod model;
pub mod transport;
