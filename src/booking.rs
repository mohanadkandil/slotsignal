use crate::{config::Result, BERLIN, SERVICE_ID, WEB_ID};
use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};

pub trait Feed {
    fn dates(&mut self, first: &str, now: DateTime<Utc>) -> Result<Vec<String>>;
    fn times(&mut self, date: &str, now: DateTime<Utc>) -> Result<Vec<String>>;
}

pub struct Booking {
    agent: ureq::Agent,
    pub base_url: String,
    parameters: BTreeMap<String, String>,
    loaded_at: i64,
}

#[derive(Deserialize)]
struct Slot {
    start: String,
    #[serde(default)]
    end: String,
    available: Option<i64>,
    wl: Option<i64>,
    #[serde(default)]
    calendarid: i64,
    #[serde(default)]
    idandtimeslot: String,
}

impl Default for Booking {
    fn default() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .into(),
            base_url: "https://www.etermin.net/api/".into(),
            parameters: BTreeMap::new(),
            loaded_at: 0,
        }
    }
}

fn parameter(value: &Value) -> Result<String> {
    match value {
        Value::Bool(v) => Ok(v.to_string()),
        Value::Number(v) => {
            if let Some(number) = v.as_i64() {
                return Ok(number.to_string());
            }
            if let Some(number) = v.as_f64().filter(|n| n.fract() == 0.0) {
                return Ok((number as i64).to_string());
            }
            Ok(v.to_string())
        }
        _ => Err("Booking setting is missing or invalid".into()),
    }
}

impl Booking {
    fn request(&self, path: &str, query: &BTreeMap<String, String>) -> Result<Value> {
        let mut address = url::Url::parse(&format!("{}{path}", self.base_url))
            .map_err(|_| "Invalid booking endpoint")?;
        address.query_pairs_mut().extend_pairs(query);
        self.agent
            .get(address.as_str())
            .header("webid", WEB_ID)
            .header("Accept", "application/json")
            .header("Pragma", "no-cache")
            .header("User-Agent", "SlotSignal/0.2")
            .call()
            .map_err(|_| format!("Appointment website request failed: {path}"))?
            .body_mut()
            .read_json()
            .map_err(|_| format!("Unexpected appointment response: {path}"))
    }

    fn load_parameters(&mut self, now: DateTime<Utc>) -> Result<()> {
        if !self.parameters.is_empty() && now.timestamp() - self.loaded_at < 3600 {
            return Ok(());
        }
        let settings = self.request("settingbs", &BTreeMap::from([("t".into(), "".into())]))?;
        let array = settings
            .as_array()
            .filter(|a| a.len() == 1)
            .ok_or("Booking settings unavailable")?;
        let settings = &array[0];
        let services = self.request(
            "servicegroupservice",
            &BTreeMap::from([
                ("cache".into(), "1".into()),
                ("w".into(), WEB_ID.into()),
                ("v".into(), parameter(&settings["vservices"])?),
                ("lang".into(), "en".into()),
            ]),
        )?;
        let service = services
            .as_array()
            .ok_or("Invalid service response")?
            .iter()
            .find(|s| s["sid"].as_i64() == Some(SERVICE_ID))
            .ok_or("Renewal service is missing")?;
        if service["s"].as_str().map(str::trim) != Some("extension of a residence permit") {
            return Err("Renewal service changed. Manual review is needed.".into());
        }
        if service["senabled"] != true || service["sgenabled"] != true {
            return Err("Renewal service is disabled".into());
        }
        let inherited = |key: &str, fallback: &str| -> &Value {
            match service.get(key) {
                Some(v) if !v.is_null() && v.as_f64() != Some(-1.0) => v,
                _ => &settings[fallback],
            }
        };
        let show = inherited("showavcap", "showavcap");
        let mut params = BTreeMap::from([
            ("serviceid".into(), SERVICE_ID.to_string()),
            ("capacity".into(), "1".into()),
            ("caching".into(), "false".into()),
            ("slottype".into(), "0".into()),
            ("oneoff".into(), "null".into()),
            ("calendarid".into(), "".into()),
            ("duration".into(), parameter(&service["duration"])?),
            ("cluster".into(), parameter(&settings["cluster"])?),
            (
                "fillcalendarstrategy".into(),
                parameter(inherited("fcs", "fillcalendarstrategy"))?,
            ),
            ("msdcm".into(), parameter(inherited("msdcm", "msdcm"))?),
            (
                "showavcap".into(),
                (show == true || show.as_i64() == Some(1)).to_string(),
            ),
        ]);
        for key in ["appfuture", "appdeadline", "appdeadlinewm"] {
            params.insert(key.into(), parameter(&settings[key])?);
        }
        self.parameters = params;
        self.loaded_at = now.timestamp();
        Ok(())
    }

    fn rows(&mut self, date: &str, ranged: bool, now: DateTime<Utc>) -> Result<Vec<Slot>> {
        self.load_parameters(now)?;
        let mut params = self.parameters.clone();
        params.insert("date".into(), date.into());
        if ranged {
            params.insert("rangesearch".into(), "1".into());
        } else {
            params.insert("tz".into(), "W. Europe Standard Time".into());
            params.insert("tzaccount".into(), "W. Europe Standard Time".into());
        }
        serde_json::from_value(self.request("timeslots", &params)?)
            .map_err(|_| "Availability response format changed".into())
    }
}

impl Feed for Booking {
    fn dates(&mut self, first: &str, now: DateTime<Utc>) -> Result<Vec<String>> {
        let mut dates = BTreeSet::new();
        for row in self.rows(first, true, now)? {
            let date = row.start.get(..10).ok_or("Invalid appointment date")?;
            NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| "Invalid appointment date")?;
            let available = row.available.ok_or("Availability field missing")?;
            let waitlist = row.wl.ok_or("Waitlist field missing")?;
            if available == 1 && waitlist == 0 {
                dates.insert(date.to_owned());
            }
        }
        Ok(dates.into_iter().collect())
    }

    fn times(&mut self, date: &str, now: DateTime<Utc>) -> Result<Vec<String>> {
        let mut times = BTreeSet::new();
        for row in self.rows(date, false, now)? {
            let local = |text: &str| -> Result<DateTime<Utc>> {
                let naive = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S%.f")
                    .map_err(|_| "Invalid appointment time")?;
                BERLIN
                    .from_local_datetime(&naive)
                    .single()
                    .map(|t| t.with_timezone(&Utc))
                    .ok_or("Ambiguous appointment time".into())
            };
            let start = local(&row.start)?;
            let end = local(&row.end)?;
            let waitlist = row.wl.ok_or("Waitlist field missing")?;
            if row.idandtimeslot.is_empty() {
                return Err("Time-slot ID missing".into());
            }
            // Daily rows are bookable slots. Their 'available' field is zero.
            if start.with_timezone(&BERLIN).format("%Y-%m-%d").to_string() == date
                && start > now
                && end > start
                && waitlist == 0
                && row.calendarid > 0
                && row.idandtimeslot != "0"
            {
                times.insert(start.with_timezone(&BERLIN).format("%H:%M").to_string());
            }
        }
        Ok(times.into_iter().collect())
    }
}
