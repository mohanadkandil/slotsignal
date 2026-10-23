mod booking;
mod config;
mod monitor;
mod telegram;

use booking::{Booking, Feed};
use chrono::{Timelike, Utc};
use config::{Config, Result};
use monitor::{Monitor, State};
use nix::unistd::Uid;
#[cfg(target_os = "linux")]
use nix::unistd::{chown, setgid, setgroups, setuid, Gid};
use rand::Rng;
#[cfg(test)]
use serde_json::json;
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};
use telegram::{Message, Messenger, Telegram};

const WEB_ID: &str = "qtermin-stadtheilbronn-abh";
const SERVICE_ID: i64 = 144512;
const BOOKING_URL: &str =
    "https://www.etermin.net/qtermin-stadtheilbronn-abh/serviceid/144512?lang=en";
const BERLIN: chrono_tz::Tz = chrono_tz::Europe::Berlin;

fn value<'a>(env: &'a HashMap<String, String>, key: &str, fallback: &'a str) -> &'a str {
    env.get(key).map(String::as_str).unwrap_or(fallback)
}

fn initialize_data(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if Uid::effective().is_root() {
        #[cfg(not(target_os = "linux"))]
        return Err("Run SlotSignal as a normal user on this platform".into());
        #[cfg(target_os = "linux")]
        {
            chown(dir, Some(Uid::from_raw(10001)), Some(Gid::from_raw(10001)))
                .map_err(|e| e.to_string())?;
            setgroups(&[])
                .and_then(|()| setgid(Gid::from_raw(10001)))
                .and_then(|()| setuid(Uid::from_raw(10001)))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

fn start_health(dir: PathBuf, port: &str, stop: Arc<AtomicBool>) -> Result<()> {
    if port.is_empty() {
        return Ok(());
    }
    let server = tiny_http::Server::http(format!("0.0.0.0:{port}"))
        .map_err(|_| "Cannot start health listener")?;
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Ok(Some(request)) = server.recv_timeout(Duration::from_secs(1)) {
                let (code, body) = if request.url() != "/health" {
                    (404, "{}")
                } else if State::load(&dir).is_ok_and(|state| state.healthy(Utc::now())) {
                    (200, "{\"healthy\":true}")
                } else {
                    (503, "{\"healthy\":false}")
                };
                let response = tiny_http::Response::from_string(body)
                    .with_status_code(code)
                    .with_header(
                        tiny_http::Header::from_bytes("Content-Type", "application/json")
                            .expect("valid header"),
                    );
                let _ = request.respond(response);
            }
        }
    });
    Ok(())
}

fn handle_command<F: Feed>(monitor: &mut Monitor<F, &Telegram>, message: &Message) -> Result<()> {
    let now = Utc::now();
    if !monitor.messenger.authorized(message) || now.timestamp() - message.date > 600 {
        return Ok(());
    }
    let command = message
        .text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .split('@')
        .next()
        .unwrap_or("");
    let stamp = now.timestamp() as f64;
    match command {
        "/test" if stamp - monitor.state.last_test_at >= 30.0 => {
            monitor.test(now)?;
            monitor.state.last_test_at = stamp;
        }
        "/check" => {
            if stamp - monitor.state.last_command_check_at >= 60.0 {
                monitor.check(now, true);
                monitor.state.last_command_check_at = stamp;
            }
            monitor.messenger.send(&monitor.status(Utc::now()))?;
        }
        "/start" | "/status" | "/help" => {
            monitor.messenger.send(&monitor.status(now))?;
        }
        _ => {}
    }
    Ok(())
}

fn sleep(stop: &AtomicBool, seconds: u64) {
    for _ in 0..seconds * 10 {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn serve(
    config: Config,
    telegram: &Telegram,
    dir: &Path,
    health_port: &str,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    initialize_data(dir)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join(".lock"))
        .map_err(|e| e.to_string())?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .map_err(|_| "Another SlotSignal process owns this data directory")?;
    let mut monitor = Monitor {
        config,
        state: State::load(dir)?,
        feed: Booking::default(),
        messenger: telegram,
    };
    start_health(dir.to_path_buf(), health_port, stop.clone())?;
    let username = telegram.setup()?;
    println!(
        "SlotSignal Rust {} started: @{username} (uid {})",
        env!("CARGO_PKG_VERSION"),
        Uid::effective()
    );
    monitor.state.started_at = Utc::now().with_timezone(&BERLIN).to_rfc3339();
    let (mut startup_sent, mut first_check) = (false, true);
    while !stop.load(Ordering::Relaxed) {
        monitor.check(Utc::now(), first_check);
        first_check = false;
        if stop.load(Ordering::Relaxed) {
            break;
        }
        monitor.state.loop_at = Utc::now().timestamp() as f64;
        monitor.state.save(dir)?;
        let now = Utc::now().with_timezone(&BERLIN);
        let today = now.format("%Y-%m-%d").to_string();
        let communication = (|| -> Result<()> {
            if !startup_sent {
                telegram.send(&format!("SERVER STARTED\n\n{}", monitor.status(Utc::now())))?;
                startup_sent = true;
                monitor.state.heartbeat_date = today.clone();
            }
            if now.hour() >= 9 && monitor.state.heartbeat_date != today {
                telegram.send(&format!(
                    "DAILY HEALTH CHECK\n\n{}",
                    monitor.status(Utc::now())
                ))?;
                monitor.state.heartbeat_date = today;
            }
            let until_check = monitor.state.next_attempt - Utc::now().timestamp() as f64;
            let timeout = if until_check > 0.0 {
                (until_check as u64).clamp(1, 20)
            } else {
                20
            };
            let updates = telegram.poll(monitor.state.telegram_offset, timeout)?;
            monitor.state.last_telegram_poll = Utc::now().with_timezone(&BERLIN).to_rfc3339();
            for update in updates {
                if let Some(message) = &update.message {
                    handle_command(&mut monitor, message)?;
                }
                monitor.state.telegram_offset = update.update_id + 1;
            }
            Ok(())
        })();
        if stop.load(Ordering::Relaxed) {
            break;
        }
        match communication {
            Ok(()) => monitor.state.telegram_error = None,
            Err(error) => {
                monitor.state.telegram_error = Some(error);
                eprintln!("Telegram communication failed. Retrying in 20 seconds.");
                sleep(&stop, 20);
            }
        }
        monitor.state.loop_at = Utc::now().timestamp() as f64;
        monitor.state.save(dir)?;
    }
    monitor.state.save(dir)
}

fn pair(telegram: &mut Telegram, path: &Path, stop: &AtomicBool) -> Result<()> {
    let username = telegram.setup()?;
    let challenge: [u8; 24] = rand::rng().random();
    let nonce: String = challenge.iter().map(|byte| format!("{byte:02x}")).collect();
    println!("Open this link in your private Telegram chat and press Start:\nhttps://t.me/{username}?start={nonce}");
    let expected = format!("/start {nonce}");
    let deadline = Instant::now() + Duration::from_secs(180);
    let mut offset = 0;
    while Instant::now() < deadline && !stop.load(Ordering::Relaxed) {
        for update in telegram.poll(offset, 20)? {
            offset = update.update_id + 1;
            let Some(message) = update.message else {
                continue;
            };
            if message.chat.kind == "private"
                && message.chat.id > 0
                && message.chat.id == message.from.id
                && !message.from.is_bot
                && message.text == expected
            {
                telegram.chat_id = message.chat.id;
                let mut options = fs::OpenOptions::new();
                options.create(true).write(true).truncate(true).mode(0o600);
                use std::io::Write;
                options
                    .open(path)
                    .and_then(|mut file| file.write_all(telegram.credentials().as_bytes()))
                    .map_err(|e| e.to_string())?;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                    .map_err(|e| e.to_string())?;
                println!("Private chat paired. Credentials saved without printing them.");
                return Ok(());
            }
        }
    }
    Err("Pairing timed out".into())
}

fn run() -> Result<()> {
    let env_path = std::env::var("ALERT_ENV_FILE").unwrap_or(".env".into());
    let env = config::environment(Path::new(&env_path))?;
    let command = std::env::args().nth(1).unwrap_or("serve".into());
    let dir = Path::new(value(&env, "ALERT_DATA_DIR", "data"));
    if command == "healthcheck" {
        return if State::load(dir)?.healthy(Utc::now()) {
            Ok(())
        } else {
            Err("Monitor is unhealthy".into())
        };
    }
    let config = Config::load(Path::new(value(&env, "ALERT_CONFIG", "config.json")))?;
    let mut telegram = Telegram::new(
        value(&env, "TELEGRAM_BOT_TOKEN", "").into(),
        value(&env, "TELEGRAM_CHAT_ID", "0")
            .parse()
            .map_err(|_| "Invalid Telegram chat ID")?,
    )?;
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)).map_err(|e| e.to_string())?;
    match command.as_str() {
        "serve" => serve(
            config,
            &telegram,
            dir,
            value(&env, "ALERT_HEALTH_PORT", ""),
            stop,
        ),
        "pair" => pair(&mut telegram, Path::new(&env_path), &stop),
        "status" => {
            let monitor = Monitor {
                config,
                state: State::load(dir)?,
                feed: Booking::default(),
                messenger: &telegram,
            };
            println!("{}", monitor.status(Utc::now()));
            Ok(())
        }
        "test" => {
            let monitor = Monitor {
                config,
                state: State::default(),
                feed: Booking::default(),
                messenger: &telegram,
            };
            monitor.test(Utc::now())?;
            println!("Telegram accepted the test message.");
            Ok(())
        }
        "check" => {
            let mut monitor = Monitor {
                config,
                state: State::default(),
                feed: Booking::default(),
                messenger: &telegram,
            };
            monitor.check(Utc::now(), true);
            println!("{}", monitor.status(Utc::now()));
            if monitor.state.status == "error" {
                Err("Availability check failed".into())
            } else {
                Ok(())
            }
        }
        _ => Err("Commands: serve, status, check, test, pair, healthcheck".into()),
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests;
