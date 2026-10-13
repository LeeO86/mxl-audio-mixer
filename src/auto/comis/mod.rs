//! Com-IS-Assist's control core (Andreas Weiss, MIT, `LICENSE` here; its
//! `crates/com-is-assist-core/src/automix`, commit 5bdd96b): the commentary/bed
//! ("COM/IS") ratio control behind the mixer's ratio ducks (§8.2, §8.4).
//!
//! The bed is reduced so that the commentary sits `target_ratio_lu` above it
//! (`overvoice_ratio_lu` while the bed carries speech too), feed-forward from the
//! dry loudness of both: required reduction = target − (COM − BED). A cascade of
//! three trims does it: slow from the bed's short-term loudness (3 s), mid from its
//! momentary loudness (400 ms), fast from ~50 ms; the attack and release emerge
//! from them, `speed` scales all. Speech in the bed while the commentary is silent
//! releases four times as fast (interview passthrough); a release eases in over
//! 0.8 s so that sentence pauses pass without movement.
//!
//! The files are copied with their documentation (which records the measurements
//! behind every constant); each says what was changed. The loudness and the VAD
//! are the mixer's.

pub mod engine;
pub mod envelope;
pub mod loop_bank;
pub mod mix_state;

/// A loudness at or below this is silence, not a level (Com-IS-Assist's
/// `Ebur128Meter::NEGATIVE_INFINITY_DB`).
pub const SILENCE_LUFS: f64 = -100.0;

#[cfg(test)]
mod tests {
    mod engine;
    mod envelope;
}
