use super::*;
use crate::{
    monitor::Opening,
    telegram::{Chat, User},
};
use chrono::{DateTime, Duration as ChronoDuration, TimeZone};
use std::{cell::RefCell, collections::BTreeMap};

fn now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 23, 10, 0, 0).unwrap()
}
fn config() -> Config {
    Config {
        first_date: "2026-10-23".into(),
        last_date: "2026-11-14".into(),
        interval_seconds: 300,
        reminder_seconds: 1800,
    }
}

#[derive(Default)]
struct FakeFeed {
    days: Vec<String>,
    times: BTreeMap<String, Vec<String>>,
    fail: bool,
    requests: usize,
}
impl Feed for FakeFeed {
    fn dates(&mut self, _: &str, _: DateTime<Utc>) -> Result<Vec<String>> {
        self.requests += 1;
        if self.fail {
            Err("Website unavailable".into())
        } else {
            Ok(self.days.clone())
        }
    }
    fn times(&mut self, date: &str, _: DateTime<Utc>) -> Result<Vec<String>> {
        Ok(self.times.get(date).cloned().unwrap_or_default())
    }
}
#[derive(Default)]
struct FakeMessenger {
    sent: RefCell<Vec<String>>,
    fail: bool,
}
impl Messenger for FakeMessenger {
    fn send(&self, text: &str) -> Result<i64> {
        if self.fail {
            return Err("Telegram delivery failed".into());
        }
        self.sent.borrow_mut().push(text.into());
        Ok(77)
    }
}
fn monitor() -> Monitor<FakeFeed, FakeMessenger> {
    Monitor {
        config: config(),
        state: State::default(),
        feed: FakeFeed::default(),
        messenger: FakeMessenger::default(),
    }
}
fn opening(m: &mut Monitor<FakeFeed, FakeMessenger>, day: &str) {
    m.feed.days = vec![day.into()];
    m.feed.times.insert(day.into(), vec!["09:00".into()]);
}

#[test]
fn confirms_times_before_sending_and_suppresses_duplicates() {
    let mut m = monitor();
    opening(&mut m, "2026-11-02");
    m.check(now(), true);
    m.check(now() + ChronoDuration::minutes(5), true);
    assert_eq!(m.state.status, "available");
    assert_eq!(m.messenger.sent.borrow().len(), 1);
    assert_eq!(m.state.telegram_message_id, 77);
    assert!(m.messenger.sent.borrow()[0].contains("2026-11-02"));
    assert!(m.messenger.sent.borrow()[0].contains("09:00"));
}

#[test]
fn reminds_after_thirty_minutes() {
    let mut m = monitor();
    opening(&mut m, "2026-11-02");
    m.check(now(), true);
    m.check(now() + ChronoDuration::minutes(30), true);
    assert_eq!(m.messenger.sent.borrow().len(), 2);
}

#[test]
fn ignores_dates_outside_window_and_disappeared_slots() {
    let mut m = monitor();
    opening(&mut m, "2026-11-15");
    m.check(now(), true);
    assert_eq!(m.state.status, "watching");
    opening(&mut m, "2026-11-02");
    m.feed.times.clear();
    m.check(now(), true);
    assert_eq!(m.state.status, "watching");
    assert!(m.messenger.sent.borrow().is_empty());
}

#[test]
fn checks_later_days_if_an_earlier_slot_disappears() {
    let mut m = monitor();
    opening(&mut m, "2026-11-03");
    m.feed.days.insert(0, "2026-11-02".into());
    m.check(now(), true);
    assert_eq!(m.state.matches[0].date, "2026-11-03");
}

#[test]
fn expiry_makes_no_website_requests() {
    let mut m = monitor();
    m.check(now() + ChronoDuration::days(30), true);
    assert_eq!(m.state.status, "expired");
    assert_eq!(m.feed.requests, 0);
}

#[test]
fn waits_for_the_next_scheduled_check() {
    let mut m = monitor();
    m.check(now(), false);
    m.check(now() + ChronoDuration::seconds(299), false);
    assert_eq!(m.feed.requests, 1);
    m.check(now() + ChronoDuration::seconds(300), false);
    assert_eq!(m.feed.requests, 2);
}

#[test]
fn failed_delivery_is_retried_and_never_recorded_as_delivered() {
    let mut m = monitor();
    opening(&mut m, "2026-11-02");
    m.messenger.fail = true;
    for n in 0..3 {
        m.check(now() + ChronoDuration::minutes(n), true);
    }
    assert_eq!(m.state.status, "error");
    assert_eq!(m.state.consecutive_errors, 3);
    assert!(m.state.notified_date.is_none());
    assert_eq!(m.state.telegram_message_id, 0);
    assert_eq!(
        m.state.next_attempt,
        (now().timestamp() + 120 + 1200) as f64
    );
}

#[test]
fn three_failures_send_one_attention_message() {
    let mut m = monitor();
    m.feed.fail = true;
    for n in 0..4 {
        m.check(now() + ChronoDuration::minutes(n), true);
    }
    assert_eq!(m.messenger.sent.borrow().len(), 1);
    assert!(m.messenger.sent.borrow()[0].contains("needs attention"));
}

#[test]
fn example_alert_cannot_be_mistaken_for_an_opening() {
    let m = monitor();
    m.test(now()).unwrap();
    let sent = m.messenger.sent.borrow();
    assert!(sent[0].contains("TEST ONLY"));
    assert!(sent[0].contains("not an available appointment"));
    assert!(sent[0].contains("Runtime: Rust"));
}

#[test]
fn reads_existing_state_and_keeps_alert_history() {
    let state: State = serde_json::from_value(json!({
        "status":"watching", "error":null, "earliest_available_date":null, "matches":[],
        "last_success":"2026-10-23T17:43:48.236040+02:00", "successful_checks":7,
        "notified_date":"2026-11-02", "notified_at":1792768325.584534,
        "telegram_offset":123, "parameters":{"serviceid":144512}
    }))
    .unwrap();
    assert_eq!(state.successful_checks, 7);
    assert_eq!(state.telegram_offset, 123);
    assert_eq!(state.notified_date.as_deref(), Some("2026-11-02"));
}

#[test]
fn health_requires_fresh_website_checks_and_telegram_polls() {
    let mut state = State {
        status: "watching".into(),
        loop_at: now().timestamp() as f64,
        last_success: now().to_rfc3339(),
        last_telegram_poll: now().to_rfc3339(),
        ..State::default()
    };
    assert!(state.healthy(now()));
    state.last_success = (now() - ChronoDuration::hours(1)).to_rfc3339();
    assert!(!state.healthy(now()));
    state.status = "expired".into();
    assert!(state.healthy(now()));
    state.last_telegram_poll.clear();
    assert!(!state.healthy(now()));
}

fn message(command: &str, id: i64) -> Message {
    Message {
        text: command.into(),
        date: Utc::now().timestamp(),
        chat: Chat {
            id,
            kind: "private".into(),
        },
        from: User { id, is_bot: false },
    }
}

#[test]
fn only_the_paired_private_chat_can_use_commands() {
    let telegram = Telegram::new("12345:FAKE_TEST_TOKEN".into(), 12345).unwrap();
    assert!(telegram.authorized(&message("/status", 12345)));
    assert!(!telegram.authorized(&message("/status", 67890)));
    let mut group = message("/status", 12345);
    group.chat.kind = "group".into();
    assert!(!telegram.authorized(&group));
    let mut impostor = message("/check", 12345);
    impostor.from.id = 67890;
    let mut m = Monitor {
        config: config(),
        state: State::default(),
        feed: FakeFeed::default(),
        messenger: &telegram,
    };
    handle_command(&mut m, &impostor).unwrap();
    assert_eq!(m.feed.requests, 0);
}

fn mock_server(
    count: usize,
    handler: impl Fn(&mut tiny_http::Request) -> serde_json::Value + Send + 'static,
) -> (String, thread::JoinHandle<()>) {
    let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
    let address = format!("http://{}", server.server_addr());
    let task = thread::spawn(move || {
        for _ in 0..count {
            let mut request = server
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .expect("expected API request");
            let response = handler(&mut request).to_string();
            request
                .respond(tiny_http::Response::from_string(response).with_header(
                    tiny_http::Header::from_bytes("Content-Type", "application/json").unwrap(),
                ))
                .unwrap();
        }
    });
    (address, task)
}

#[test]
fn telegram_sends_the_date_and_booking_button() {
    let (address, task) = mock_server(1, |request| {
        let payload: serde_json::Value = serde_json::from_reader(request.as_reader()).unwrap();
        assert_eq!(payload["chat_id"], "12345");
        assert!(payload["text"].as_str().unwrap().contains("2026-11-02"));
        assert_eq!(
            payload["reply_markup"]["inline_keyboard"][0][0]["url"],
            BOOKING_URL
        );
        json!({"ok":true,"result":{"message_id":77}})
    });
    let mut telegram = Telegram::new("12345:FAKE_TEST_TOKEN".into(), 12345).unwrap();
    telegram.base_url = address;
    assert_eq!(telegram.send("Date: 2026-11-02 at 09:00").unwrap(), 77);
    task.join().unwrap();
}

#[test]
fn telegram_errors_never_contain_the_token() {
    let token = "12345:FAKE_TEST_TOKEN";
    let mut telegram = Telegram::new(token.into(), 12345).unwrap();
    telegram.base_url = "http://127.0.0.1:9".into();
    let error = telegram.send("test").unwrap_err();
    assert!(!error.contains(token));
    assert!(!error.contains("http://"));
}

fn booking_fixture(request: &mut tiny_http::Request) -> serde_json::Value {
    assert!(request
        .headers()
        .iter()
        .any(|header| header.field.equiv("webid") && header.value.as_str() == WEB_ID));
    let address = url::Url::parse(&format!("http://localhost{}", request.url())).unwrap();
    let query: BTreeMap<_, _> = address.query_pairs().into_owned().collect();
    match address.path() {
        "/api/settingbs" => {
            json!([{"vservices":1,"cluster":false,"fillcalendarstrategy":0,"showavcap":false,"msdcm":0,"appfuture":270,"appdeadline":480,"appdeadlinewm":1}])
        }
        "/api/servicegroupservice" => {
            json!([{"sid":144512,"s":"extension of a residence permit","senabled":true,"sgenabled":true,"duration":20,"fcs":-1,"msdcm":-1,"showavcap":-1}])
        }
        "/api/timeslots" => {
            assert_eq!(query["capacity"], "1");
            assert_eq!(query["appdeadline"], "480");
            assert_eq!(query["duration"], "20");
            if query.contains_key("rangesearch") {
                json!([{"start":"2026-11-02T00:00:00","available":1,"wl":0}])
            } else {
                json!([{"start":"2026-11-02T09:00:00","end":"2026-11-02T09:20:00","available":0,"wl":0,"calendarid":1,"idandtimeslot":"1|example"}])
            }
        }
        _ => panic!("Unexpected API path"),
    }
}

#[test]
fn booking_uses_website_constraints_and_confirms_daily_times() {
    let (address, task) = mock_server(4, booking_fixture);
    let mut booking = Booking::default();
    booking.base_url = address + "/api/";
    assert_eq!(
        booking.dates("2026-10-23", now()).unwrap(),
        vec!["2026-11-02"]
    );
    assert_eq!(booking.times("2026-11-02", now()).unwrap(), vec!["09:00"]);
    task.join().unwrap();
}

#[test]
fn malformed_website_response_is_an_error() {
    let (address, task) = mock_server(3, |request| {
        if request.url().starts_with("/api/timeslots") {
            json!([{"unexpected":true}])
        } else {
            booking_fixture(request)
        }
    });
    let mut booking = Booking::default();
    booking.base_url = address + "/api/";
    assert!(booking.dates("2026-10-23", now()).is_err());
    task.join().unwrap();
}

#[test]
fn saves_and_reloads_delivery_state() {
    let dir = std::env::temp_dir().join(format!("slotsignal-test-{}", rand::random::<u64>()));
    fs::create_dir(&dir).unwrap();
    let state = State {
        matches: vec![Opening {
            date: "2026-11-02".into(),
            times: vec!["09:00".into()],
        }],
        telegram_offset: 42,
        ..State::default()
    };
    state.save(&dir).unwrap();
    let restored = State::load(&dir).unwrap();
    assert_eq!(state.matches, restored.matches);
    assert_eq!(restored.telegram_offset, 42);
    fs::remove_dir_all(dir).unwrap();
}
