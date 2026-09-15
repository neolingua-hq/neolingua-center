//! Browser-ready cache: remuxed MP4 + EN/FR WebVTT beside app data.

mod ffmpeg;
mod registry;
mod subs;
mod types;

pub use registry::{enrich_catalog_prep, PrepRegistry};
pub use types::{EpisodePrep, PrepStatus, SubTrackSource};
