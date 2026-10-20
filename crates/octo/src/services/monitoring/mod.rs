//! Monitoring: the listening log (`listening.json`) the dashboard's Monitoring page reads. The
//! log and its summary are `octo_core::monitoring`.

pub mod listening_store;

pub use listening_store::ListeningStore;
