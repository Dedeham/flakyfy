//! "Tidy artists": bring the files already in the library in line with how a new download is
//! written. The plan is `planner`; the rest reads the library, writes, waits for Navidrome and
//! moves, with a journal for Undo.

pub mod planner;
