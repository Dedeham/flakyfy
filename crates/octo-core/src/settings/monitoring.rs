//! The `Monitoring` section: the listening log behind the dashboard's Monitoring page.

use serde::{Deserialize, Serialize};

/// The longest a listening log may be kept, in days (ten years).
pub const MAX_RETENTION_DAYS: u32 = 3650;

/// Monitoring's settings. Off by default: with `Enabled` false no play is recorded.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct MonitoringSettings {
    /// Records completed plays per listener, day, genre, platform and app. Environment
    /// variable: MONITORING__ENABLED (docker-compose maps MONITORING_ENABLED).
    pub enabled: bool,
    /// Days of listening kept; older days are dropped. Environment variable:
    /// MONITORING__RETENTIONDAYS (docker-compose maps MONITORING_RETENTION_DAYS).
    pub retention_days: u32,
}

impl Default for MonitoringSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            retention_days: 365,
        }
    }
}

impl MonitoringSettings {
    /// `RetentionDays` between one day and [`MAX_RETENTION_DAYS`].
    pub fn effective_retention_days(&self) -> u32 {
        self.retention_days.clamp(1, MAX_RETENTION_DAYS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_is_kept_between_a_day_and_ten_years() {
        let mut settings = MonitoringSettings::default();
        assert_eq!(settings.effective_retention_days(), 365);
        settings.retention_days = 0;
        assert_eq!(settings.effective_retention_days(), 1);
        settings.retention_days = 100_000;
        assert_eq!(settings.effective_retention_days(), MAX_RETENTION_DAYS);
    }
}
