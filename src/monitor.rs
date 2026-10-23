use crate::{
    booking::Feed,
    config::{Config, Result},
    telegram::Messenger,
    BERLIN,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Opening {
    pub date: String,
    pub times: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub status: String,
    pub runtime: String,
    pub last_run: String,
    pub last_success: String,
    pub earliest_available_date: Option<String>,
    pub matches: Vec<Opening>,
    pub successful_checks: u64,
    pub consecutive_errors: u32,
    pub error: Option<String>,
    pub next_attempt: f64,
    pub notified_date: Option<String>,
    pub notified_at: f64,
    pub error_notified_at: f64,
    pub last_notification_at: String,
    pub telegram_message_id: i64,
    pub telegram_offset: i64,
    pub heartbeat_date: String,
    pub last_test_at: f64,
    pub last_command_check_at: f64,
    pub loop_at: f64,
    pub last_telegram_poll: String,
    pub telegram_error: Option<String>,
    pub started_at: String,
}

impl State {
    pub fn load(dir: &Path) -> Result<Self> {
        match fs::read(dir.join("status.json")) {
            Ok(data) => {
                serde_json::from_slice(&data).map_err(|_| "Cannot read monitor state".into())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }
    pub fn save(&self, dir: &Path) -> Result<()> {
        let data = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let path = dir.join("status.tmp");
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| e.to_string())?;
        file.write_all(&data)
            .and_then(|()| file.sync_all())
            .map_err(|e| e.to_string())?;
        fs::rename(path, dir.join("status.json")).map_err(|e| e.to_string())
    }
    pub fn healthy(&self, now: DateTime<Utc>) -> bool {
        let recent = |text: &str, limit: i64| {
            DateTime::parse_from_rfc3339(text)
                .is_ok_and(|date| (now.timestamp() - date.timestamp()).abs() <= limit)
        };
        if (now.timestamp() as f64 - self.loop_at).abs() > 150.0
            || !recent(&self.last_telegram_poll, 150)
        {
            return false;
        }
        self.status == "expired"
            || (recent(&self.last_success, 1200) && self.consecutive_errors < 3)
    }
}

pub struct Monitor<F: Feed, M: Messenger> {
    pub config: Config,
    pub state: State,
    pub feed: F,
    pub messenger: M,
}

impl<T: Messenger + ?Sized> Messenger for &T {
    fn send(&self, text: &str) -> Result<i64> {
        (**self).send(text)
    }
}

impl<F: Feed, M: Messenger> Monitor<F, M> {
    pub fn check(&mut self, now: DateTime<Utc>, force: bool) {
        self.state.runtime = "rust".into();
        self.state.last_run = now.with_timezone(&BERLIN).to_rfc3339();
        let today = now.with_timezone(&BERLIN).format("%Y-%m-%d").to_string();
        let first = today.max(self.config.first_date.clone());
        if first > self.config.last_date {
            self.state.status = "expired".into();
            self.state.error = None;
            self.state.matches.clear();
            return;
        }
        let stamp = now.timestamp() as f64;
        if !force && stamp < self.state.next_attempt {
            return;
        }
        let previous = self.state.consecutive_errors;
        if let Err(error) = self.check_available(now, &first) {
            self.state.status = "error".into();
            self.state.error = Some(error);
            self.state.consecutive_errors = previous.saturating_add(1);
            let delay = 300_i64
                .saturating_mul(1_i64 << self.state.consecutive_errors.saturating_sub(1).min(4))
                .min(3600);
            self.state.next_attempt = stamp + delay as f64;
            if self.state.consecutive_errors >= 3 && stamp - self.state.error_notified_at >= 86400.0
                && self.messenger.send("SlotSignal needs attention\n\nThree checks or alert deliveries failed. Send /status for details.").is_ok() {
                self.state.error_notified_at = stamp;
            }
        }
    }

    fn check_available(&mut self, now: DateTime<Utc>, first: &str) -> Result<()> {
        let dates = self.feed.dates(first, now)?;
        self.state.earliest_available_date = dates.first().cloned();
        let mut matches = Vec::new();
        for date in dates {
            if date.as_str() < first || date > self.config.last_date {
                continue;
            }
            let times = self.feed.times(&date, now)?;
            if !times.is_empty() {
                matches.push(Opening { date, times });
                break;
            }
        }
        self.state.status = "watching".into();
        self.state.matches = matches;
        self.state.last_success = now.with_timezone(&BERLIN).to_rfc3339();
        self.state.successful_checks += 1;
        self.state.consecutive_errors = 0;
        self.state.error = None;
        self.state.next_attempt = (now.timestamp() + self.config.interval_seconds) as f64;
        let Some(opening) = self.state.matches.first() else {
            self.state.notified_date = None;
            return Ok(());
        };
        self.state.status = "available".into();
        if self.state.notified_date.as_ref() == Some(&opening.date)
            && now.timestamp() as f64 - self.state.notified_at < self.config.reminder_seconds as f64
        {
            return Ok(());
        }
        let text = format!("SlotSignal: appointment available\n\nDate: {}\nTimes: {} (Europe/Berlin)\nService: extension of a residence permit\nBook promptly; availability can change.",
            opening.date, opening.times.iter().take(8).cloned().collect::<Vec<_>>().join(", "));
        let id = self.messenger.send(&text)?;
        self.state.notified_date = Some(opening.date.clone());
        self.state.notified_at = now.timestamp() as f64;
        self.state.telegram_message_id = id;
        self.state.last_notification_at = now.with_timezone(&BERLIN).to_rfc3339();
        Ok(())
    }

    pub fn status(&self, now: DateTime<Utc>) -> String {
        let checked = if self.state.last_success.is_empty() {
            "not yet successful"
        } else {
            &self.state.last_success
        };
        let mut text = format!("SlotSignal | Heilbronn\nRuntime: Rust {}\nServer time: {}\nWindow: {} through {} inclusive\nChecks: every {} minutes\nState: {}\nLast successful availability check: {}\nSuccessful checks since setup: {}",
            env!("CARGO_PKG_VERSION"), now.with_timezone(&BERLIN).to_rfc3339(), self.config.first_date, self.config.last_date,
            self.config.interval_seconds / 60, self.state.status, checked, self.state.successful_checks);
        if let Some(date) = &self.state.earliest_available_date {
            text += &format!("\nEarliest date returned by website: {date}");
        }
        if self.state.status == "watching" {
            text += "\nNo matching appointments at the last successful check.";
        }
        if self.state.status == "available" {
            if let Some(opening) = self.state.matches.first() {
                text += &format!(
                    "\nOpening: {} at {}",
                    opening.date,
                    opening.times.join(", ")
                );
            }
        }
        if let Some(error) = &self.state.error {
            text += &format!("\nAttention: {error}");
        }
        if let Some(error) = &self.state.telegram_error {
            text += &format!("\nTelegram communication: {error}");
        }
        text + "\n/status = health; /check = live check; /test = example alert"
    }

    pub fn test(&self, now: DateTime<Utc>) -> Result<i64> {
        self.messenger.send(&format!("TEST ONLY: SlotSignal example alert\n\nExample date: {} at 09:00 (Europe/Berlin).\nThis is simulated, not an available appointment.\nThe booking link is real. No appointment was booked.\nRuntime: Rust {}",
            (now.with_timezone(&BERLIN) + Duration::days(1)).format("%Y-%m-%d"), env!("CARGO_PKG_VERSION")))
    }
}
