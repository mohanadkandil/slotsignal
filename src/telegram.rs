use crate::{config::Result, BOOKING_URL};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;

pub trait Messenger {
    fn send(&self, text: &str) -> Result<i64>;
}

pub struct Telegram {
    token: String,
    pub chat_id: i64,
    pub base_url: String,
    agent: ureq::Agent,
}

#[derive(Clone, Default, Deserialize)]
pub struct Chat {
    pub id: i64,
    #[serde(rename = "type", default)]
    pub kind: String,
}
#[derive(Clone, Default, Deserialize)]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub is_bot: bool,
}
#[derive(Clone, Default, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub text: String,
    pub date: i64,
    pub chat: Chat,
    pub from: User,
}
#[derive(Clone, Deserialize)]
pub struct Update {
    pub update_id: i64,
    #[serde(default)]
    pub message: Option<Message>,
}

impl Telegram {
    pub fn new(token: String, chat_id: i64) -> Result<Self> {
        let (id, key) = token
            .split_once(':')
            .ok_or("Telegram token is missing or invalid")?;
        if id.parse::<u64>().is_err()
            || key.is_empty()
            || !key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("Telegram token is missing or invalid".into());
        }
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(35)))
            .build()
            .into();
        Ok(Self {
            token,
            chat_id,
            agent,
            base_url: "https://api.telegram.org".into(),
        })
    }

    pub fn call(&self, method: &str, payload: Value) -> Result<Value> {
        let address = format!("{}/bot{}/{}", self.base_url, self.token, method);
        let mut response = self
            .agent
            .post(&address)
            .send_json(payload)
            .map_err(|_| format!("Telegram request failed: {method}"))?;
        let body: Value = response
            .body_mut()
            .read_json()
            .map_err(|_| format!("Unexpected Telegram response: {method}"))?;
        if body.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(format!("Telegram rejected {method}"));
        }
        body.get("result")
            .cloned()
            .ok_or_else(|| format!("Telegram result missing: {method}"))
    }

    pub fn setup(&self) -> Result<String> {
        let me = self.call("getMe", json!({}))?;
        let webhook = self.call("getWebhookInfo", json!({}))?;
        if webhook["url"].as_str().is_some_and(|url| !url.is_empty()) {
            return Err("This bot has a webhook. Use a dedicated bot.".into());
        }
        self.call(
            "setMyCommands",
            json!({"commands": [
                {"command":"status", "description":"Server health and last successful check"},
                {"command":"check", "description":"Check appointment availability now"},
                {"command":"test", "description":"Send a clearly marked example alert"},
                {"command":"help", "description":"Show commands and status"}
            ]}),
        )?;
        me["username"]
            .as_str()
            .map(str::to_owned)
            .ok_or("Bot username missing".into())
    }

    pub fn poll(&self, offset: i64, timeout: u64) -> Result<Vec<Update>> {
        serde_json::from_value(self.call(
            "getUpdates",
            json!({
                "offset": offset, "timeout": timeout, "allowed_updates": ["message"]
            }),
        )?)
        .map_err(|_| "Unexpected Telegram update".into())
    }

    pub fn authorized(&self, message: &Message) -> bool {
        self.chat_id > 0
            && message.chat.kind == "private"
            && message.chat.id == self.chat_id
            && message.from.id == self.chat_id
            && !message.from.is_bot
    }

    pub fn credentials(&self) -> String {
        format!(
            "TELEGRAM_BOT_TOKEN={}\nTELEGRAM_CHAT_ID={}\n",
            self.token, self.chat_id
        )
    }
}

impl Messenger for Telegram {
    fn send(&self, text: &str) -> Result<i64> {
        if self.chat_id <= 0 {
            return Err("Pair a private Telegram chat first".into());
        }
        self.call("sendMessage", json!({
            "chat_id": self.chat_id.to_string(), "text": format!("{text}\n\n{BOOKING_URL}"),
            "link_preview_options": {"is_disabled": true},
            "reply_markup": {"inline_keyboard": [[{"text":"Open renewal booking", "url": BOOKING_URL}]]}
        }))?["message_id"].as_i64().ok_or("Telegram delivery receipt missing".into())
    }
}
