use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;

pub fn all<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    fn walk<'a>(value: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
        match value {
            Value::Object(map) => {
                for (k, child) in map {
                    if k == key {
                        out.push(child);
                    }
                    walk(child, key, out);
                }
            }
            Value::Array(items) => items.iter().for_each(|child| walk(child, key, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, key, &mut out);
    out
}

pub fn first<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    all(value, key).into_iter().next()
}

pub fn first_str(value: &Value, key: &str) -> Option<String> {
    first(value, key).and_then(Value::as_str).map(String::from)
}

pub fn find_any<'a>(value: &'a Value, keys: &[&str]) -> Vec<(&'a str, &'a Value)> {
    fn walk<'a>(value: &'a Value, keys: &[&str], out: &mut Vec<(&'a str, &'a Value)>) {
        match value {
            Value::Object(map) => {
                for (k, child) in map {
                    if keys.contains(&k.as_str()) {
                        out.push((k.as_str(), child));
                    } else {
                        walk(child, keys, out);
                    }
                }
            }
            Value::Array(items) => items.iter().for_each(|child| walk(child, keys, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(value, keys, &mut out);
    out
}

pub fn text(value: &Value) -> Option<String> {
    if let Some(s) = value.get("simpleText").or_else(|| value.get("content")).and_then(Value::as_str) {
        return Some(s.to_string());
    }
    let runs = value.get("runs")?.as_array()?;
    Some(runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect())
}

pub fn parse_count(text: &str) -> Option<u64> {
    let t = text.trim().to_lowercase();
    if t.starts_with("no ") {
        return Some(0);
    }
    let number: String = t.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == ',').collect();
    if number.is_empty() {
        return None;
    }
    let rest = t[number.len()..].trim_start();
    let multiplier = if rest.starts_with("thousand") || rest.starts_with('k') {
        1e3
    } else if rest.starts_with("million") || rest.starts_with('m') {
        1e6
    } else if rest.starts_with("billion") || rest.starts_with('b') {
        1e9
    } else {
        1.0
    };
    let n: f64 = number.replace(',', "").parse().ok()?;
    Some((n * multiplier).round() as u64)
}

pub fn parse_age_hours(text: &str) -> Option<f64> {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    let pos = words.iter().position(|w| w.chars().all(|c| c.is_ascii_digit()))?;
    let n: f64 = words[pos].parse().ok()?;
    let unit = words.get(pos + 1)?;
    let hours = [("second", 1.0 / 3600.0), ("minute", 1.0 / 60.0), ("hour", 1.0), ("day", 24.0), ("week", 168.0), ("month", 730.0), ("year", 8760.0)]
        .iter()
        .find(|(name, _)| unit.starts_with(name))?
        .1;
    Some(n * hours)
}

pub fn parse_duration(text: &str) -> Option<u64> {
    if !text.contains(':') {
        return None;
    }
    text.trim().split(':').try_fold(0u64, |acc, part| part.parse::<u64>().ok().map(|n| acc * 60 + n))
}

pub fn hours_since(iso: &str) -> Option<f64> {
    let published = DateTime::parse_from_rfc3339(iso)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| NaiveDate::parse_from_str(iso, "%Y-%m-%d").ok()?.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc()))?;
    Some((Utc::now() - published).num_seconds() as f64 / 3600.0)
}

pub fn video_id(input: &str) -> Option<String> {
    let s = input.trim();
    [s.split("v=").nth(1), s.split("youtu.be/").nth(1), s.split("/shorts/").nth(1), s.split("/live/").nth(1), Some(s)]
        .into_iter()
        .flatten()
        .map(|c| c.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>())
        .find(|id| id.len() == 11)
}

pub fn normalize(text: &str) -> String {
    text.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn hashtags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace().filter(|w| w.starts_with('#') && w.len() > 1) {
        let tag = word.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_').to_lowercase();
        if tag.len() > 1 && !out.contains(&tag) {
            out.push(tag);
        }
    }
    out
}

pub fn has_timestamps(description: &str) -> bool {
    description
        .lines()
        .filter(|line| line.split_whitespace().next().and_then(parse_duration).is_some())
        .count()
        >= 3
}

pub fn split_keywords(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    for c in raw.chars() {
        match c {
            '"' => quoted = !quoted,
            ' ' if !quoted => {}
            _ => {
                current.push(c);
                continue;
            }
        }
        if !current.is_empty() {
            out.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        assert_eq!(parse_count("356,784 views"), Some(356_784));
        assert_eq!(parse_count("356K views"), Some(356_000));
        assert_eq!(parse_count("1.2M subscribers"), Some(1_200_000));
        assert_eq!(parse_count("24 thousand views"), Some(24_000));
        assert_eq!(parse_count("No views"), Some(0));
    }

    #[test]
    fn ages_and_durations() {
        assert_eq!(parse_age_hours("Streamed 2 weeks ago"), Some(336.0));
        assert_eq!(parse_age_hours("3 hours ago"), Some(3.0));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("8:01"), Some(481));
    }

    #[test]
    fn ids_and_keywords() {
        assert_eq!(video_id("https://www.youtube.com/watch?v=jNQXAC9IVRw&t=4").as_deref(), Some("jNQXAC9IVRw"));
        assert_eq!(video_id("https://youtube.com/shorts/k49vd5QxfJc").as_deref(), Some("k49vd5QxfJc"));
        assert_eq!(split_keywords("\"Roblox development\" roblox \"how to\""), vec!["Roblox development", "roblox", "how to"]);
    }
}
