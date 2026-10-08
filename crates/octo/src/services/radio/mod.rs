//! `Services/Radio`: radio's suggestion sources. What is here so far is what radio learns from
//! listening, as `radio-outcomes.json` keeps it; the sources, the blend and the learning itself
//! are group L's.

pub mod radio_outcome_store;

pub use radio_outcome_store::{OutcomeCounts, OutcomeState};
