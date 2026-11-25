//! English-only file logging for the launcher.
//!
//! Terminal output goes through the i18n system; this module never does.
//! Log lines stay in English regardless of the user's language setting so
//! files remain comparable across locales and can be attached to bug
//! reports. Errors carry a stable code so entries can be searched without
//! matching translated text.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use super::config;

/// Single append-only log file inside the shared data log directory.
const LOG_FILE: &str = "launcher.log";

static LOG: OnceLock<Mutex<Option<File>>> = OnceLock::new();

/// Open `<data>/log/launcher.log`. Logging stays silently disabled when the
/// file cannot be created; the launcher must not fail because of its own log.
pub fn init() {
    let file = config::log_dir()
        .and_then(|dir| {
            std::fs::create_dir_all(&dir)?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join(LOG_FILE))
                .map_err(config::ConfigError::from)
        })
        .ok();
    LOG.get_or_init(|| Mutex::new(file));
}

pub fn info(message: &str) {
    write_line("INFO", None, message);
}

/// `code` is a stable identifier chosen by the caller, e.g. an error code.
pub fn error(code: &str, message: &str) {
    write_line("ERROR", Some(code), message);
}

fn write_line(level: &str, code: Option<&str>, message: &str) {
    let Some(log) = LOG.get() else { return };
    let Ok(mut guard) = log.lock() else { return };
    let Some(file) = guard.as_mut() else { return };
    let code = code.map(|code| format!(" code={code}")).unwrap_or_default();
    let line = format!("{} {level}{code} {message}\n", timestamp_utc());
    let _ = file.write_all(line.as_bytes()).and_then(|_| file.flush());
}

/// UTC timestamp built from the epoch seconds directly, avoiding an extra
/// date-time dependency.
fn timestamp_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = secs / 86_400;
    let time = secs % 86_400;
    // Howard Hinnant's civil_from_days algorithm.
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096)
        / 1461;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_position = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_position + 2) / 5 + 1;
    let month = if month_position < 10 { month_position + 3 } else { month_position - 9 };
    if month <= 2 {
        year += 1;
    }
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        time / 3600,
        (time / 60) % 60,
        time % 60
    )
}
