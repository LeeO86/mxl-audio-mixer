//! The mix: signal model, channel formats and the plan compiler (§4.1, §5).

pub mod auto;
pub mod chain;
pub mod compile;
pub mod format;
pub mod logic;
pub mod model;
pub mod operations;
pub mod reformat;
pub mod room;
pub mod show;
pub mod spatial;
pub mod talkback;
pub mod tolerant;
pub mod vca;

pub use format::Format;
pub use model::Mix;
