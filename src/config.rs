use serde::Deserialize;
use std::{collections::HashMap, fs, os::unix::fs::PermissionsExt, path::Path};

pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Deserialize)]
pub struct Config {
    pub first_date: String,
    pub last_date: String,
    pub interval_seconds: i64,
    pub reminder_seconds: i64,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        for date in [&config.first_date, &config.last_date] {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .map_err(|_| "Dates must use YYYY-MM-DD")?;
        }
        if config.first_date > config.last_date {
            return Err("Last date precedes first date".into());
        }
        if config.interval_seconds < 300 || config.reminder_seconds < 300 {
            return Err("Check and reminder intervals must be at least five minutes".into());
        }
        Ok(config)
    }
}

pub fn environment(path: &Path) -> Result<HashMap<String, String>> {
    let mut values = HashMap::new();
    match fs::read_to_string(path) {
        Ok(text) => {
            if fs::metadata(path)
                .map_err(|e| e.to_string())?
                .permissions()
                .mode()
                & 0o077
                != 0
            {
                return Err("Run chmod 600 .env before loading credentials".into());
            }
            for line in text
                .lines()
                .filter(|line| !line.trim().is_empty() && !line.trim_start().starts_with('#'))
            {
                let (key, value) = line.split_once('=').ok_or("Invalid environment file")?;
                values.insert(key.to_owned(), value.to_owned());
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    values.extend(std::env::vars());
    Ok(values)
}
