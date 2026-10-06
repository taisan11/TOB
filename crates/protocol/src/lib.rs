//! Current Bedrock/NetherNet protocol types owned by TOB.

use serde::{Deserialize, Serialize};
pub mod nethernet;

/// The direction in which a datagram is travelling through the proxy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    ClientToServer,
    ServerToClient,
}

/// Protocol versions are kept explicit instead of assuming a single Bedrock release.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BedrockVersion {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}
