//! The startup checks printed to the console. The result type, the validator interface and the
//! base helpers are `flakyfy_core::validation`.

pub mod startup_validation_orchestrator;
pub mod subsonic_startup_validator;

pub use flakyfy_core::validation::{ConsoleColor, IStartupValidator, ValidationResult};
pub use startup_validation_orchestrator::StartupValidationOrchestrator;
pub use subsonic_startup_validator::SubsonicStartupValidator;
