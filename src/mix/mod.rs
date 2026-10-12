//! The mix: signal model, channel formats and the plan compiler (§4.1, §5).

pub mod chain;
pub mod compile;
pub mod format;
pub mod model;
pub mod room;
pub mod show;
pub mod talkback;
pub mod vca;

pub use format::Format;
pub use model::Mix;
