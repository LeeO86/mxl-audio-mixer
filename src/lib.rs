//! mxl-audio-mixer: a broadcast audio engine on MXL (`SPECIFICATION.md`).

pub mod analysis;
pub mod api;
pub mod config;
pub mod control;
pub mod dsp;
pub mod engine;
pub mod events;
pub mod history;
pub mod inputs;
pub mod loudness;
pub mod meters;
pub mod mix;
pub mod mxlio;
pub mod nmos;
pub mod ops;
pub mod outputs;
pub mod preview;
pub mod preview_api;
pub mod room_api;
pub mod show_api;
pub mod state;
pub mod tally;
pub mod timing;
pub mod widgets;
