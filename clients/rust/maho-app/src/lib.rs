//! Cross-platform MahoRD client orchestration.

mod abr;
pub mod agent_input;
pub mod agent_server;
mod audio;
mod clipboard;
pub mod error;
pub mod frame_queue;
mod input;
pub mod latency;
pub mod mcp_server;
mod media;
mod pairing;
mod receiver_stats;
mod session;

pub use abr::*;
pub use agent_input::*;
pub use agent_server::*;
pub use audio::*;
pub use clipboard::*;
pub use error::*;
pub use input::*;
pub use latency::*;
pub use mcp_server::*;
pub use media::*;
pub use pairing::*;
pub use receiver_stats::{
    ReceiverSnapshot, RECEIVER_REORDER_GRACE, RECEIVER_STATS_WINDOW, RECEIVER_TELEMETRY_CAPACITY,
};
pub use session::*;
