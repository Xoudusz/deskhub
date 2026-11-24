//! The board's location, clock and units ("location" message): sent once the user edits it
//! in the app (`location.enabled`), at start and on every change; the Publisher resends it
//! after each reconnect. The board ignores an identical one and restarts on a change.

use serde_json::{json, Value};

use super::Ctx;
use crate::settings::Settings;

/// None while the app leaves the board's own location alone.
pub fn location_message(s: &Settings) -> Option<Value> {
    let l = &s.location;
    l.enabled.then(|| {
        json!({
            "type": "location",
            "city": l.city.trim(),
            "lat": l.lat.trim(),
            "lon": l.lon.trim(),
            "tz": l.tz.trim(),
            "fahrenheit": l.fahrenheit,
            "h24": l.h24,
        })
    })
}

pub async fn run(mut ctx: Ctx) {
    let mut last = location_message(&ctx.settings.borrow_and_update());
    if let Some(m) = &last {
        ctx.out.publish("location", m.clone());
    }
    while ctx.settings.changed().await.is_ok() {
        let next = location_message(&ctx.settings.borrow_and_update());
        if next != last {
            if let Some(m) = &next {
                ctx.out.publish("location", m.clone());
            }
            last = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_when_enabled() {
        let mut s = Settings::default();
        assert_eq!(location_message(&s), None);
        s.location.enabled = true;
        s.location.city = " Vienna ".into();
        s.location.h24 = true;
        let m = location_message(&s).unwrap();
        assert_eq!(m["type"], "location");
        assert_eq!(m["city"], "Vienna");
        assert_eq!(m["lat"], "40.7128");
        assert_eq!(m["h24"], true);
    }
}
