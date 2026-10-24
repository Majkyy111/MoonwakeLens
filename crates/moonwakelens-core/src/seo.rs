use crate::parse::{has_timestamps, hashtags, normalize};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct SeoInput {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub tags: Vec<String>,
    pub keyword: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeoCheck {
    pub label: String,
    pub passed: bool,
    pub points: u8,
    pub max: u8,
    pub tip: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SeoReport {
    pub score: u8,
    pub checks: Vec<SeoCheck>,
    pub controversial: Vec<String>,
}

const CONTROVERSIAL: &[&str] = &[
    "abuse", "alcohol", "blood", "bomb", "casino", "cocaine", "dead body", "death", "drugs", "drunk", "free robux", "gambling", "gore", "gun", "guns", "hack",
    "hacks", "hitler", "kill", "killed", "killing", "meth", "murder", "naked", "nazi", "nsfw", "nude", "porn", "racist", "rape", "scam", "self harm", "sex",
    "sexy", "shooter", "shooting", "suicide", "terrorist", "vape", "violence", "weed",
];

pub fn controversial_words(text: &str) -> Vec<String> {
    let words = format!(" {} ", normalize(&text.replace(|c: char| !c.is_alphanumeric(), " ")));
    CONTROVERSIAL.iter().filter(|w| words.contains(&format!(" {w} "))).map(|w| w.to_string()).collect()
}

pub fn score(input: &SeoInput, suggested_tags: &[String]) -> SeoReport {
    let title = normalize(&input.title);
    let description_head: String = normalize(&input.description).chars().take(150).collect();
    let tags: Vec<String> = input.tags.iter().map(|t| normalize(t)).collect();
    let keyword = input.keyword.as_deref().map(normalize).filter(|k| !k.is_empty());
    let has = |text: &str| keyword.as_ref().is_some_and(|k| text.contains(k.as_str()));
    let title_length = input.title.chars().count();
    let tags_length: usize = input.tags.iter().map(|t| t.chars().count() + if t.contains(' ') { 3 } else { 1 }).sum();
    let hashtag_count = hashtags(&format!("{} {}", input.title, input.description)).len();
    let overlap = if suggested_tags.is_empty() {
        0.0
    } else {
        suggested_tags.iter().filter(|s| tags.contains(&normalize(s))).count() as f64 / suggested_tags.len().min(5) as f64
    };
    let flag = |ok: bool| if ok { 1.0 } else { 0.0 };
    let controversial = controversial_words(&format!("{} {} {}", input.title, input.description, input.tags.join(" ")));

    let mut checks = Vec::new();
    let mut add = |label: &str, max: u8, ratio: f64, tip: &str| {
        let ratio = ratio.clamp(0.0, 1.0);
        checks.push(SeoCheck { label: label.into(), passed: ratio >= 1.0, points: (max as f64 * ratio).round() as u8, max, tip: tip.into() });
    };
    if keyword.is_some() {
        add("Keyword in title", 20, flag(has(&title)), "Put the main keyword in the title.");
        add("Keyword in the first 30 characters of the title", 5, flag(keyword.as_ref().and_then(|k| title.find(k.as_str())).is_some_and(|i| i <= 30)), "Move the keyword to the start of the title.");
        add("Keyword in the first 150 characters of the description", 10, flag(has(&description_head)), "Mention the keyword in the first sentence of the description.");
        add("Keyword in tags", 10, flag(tags.iter().any(|t| has(t))), "Add the keyword as a tag.");
        add("Uses tags that ranking videos use", 15, overlap, "Add tags from Tag suggestions for this keyword.");
    }
    add("Title is 30-70 characters", 10, flag((30..=70).contains(&title_length)), "Keep the title between 30 and 70 characters.");
    add("Description is at least 250 characters", 10, input.description.chars().count() as f64 / 250.0, "Write a longer description (250+ characters).");
    add("5 to 20 tags", 5, flag((5..=20).contains(&tags.len())), "Use between 5 and 20 tags.");
    add("Tags fit in 500 characters", 5, flag(!tags.is_empty() && tags_length <= 500), "Keep all tags together under 500 characters.");
    add("1 to 15 hashtags", 5, flag((1..=15).contains(&hashtag_count)), "Add 1-3 relevant hashtags (never more than 15).");
    add("Chapters in the description", 5, flag(has_timestamps(&input.description)), "Add timestamps (0:00 Intro ...) to get chapters.");
    add("No controversial keywords", 5, flag(controversial.is_empty()), "Advertisers avoid some words; reword them to protect ads and reach.");

    let max: u32 = checks.iter().map(|c| c.max as u32).sum();
    let got: u32 = checks.iter().map(|c| c.points as u32).sum();
    SeoReport { score: (got as f64 * 100.0 / max as f64).round() as u8, checks, controversial }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewards_keyword_use() {
        let input = SeoInput {
            title: "Roblox game ideas that actually work in 2026".into(),
            description: "Roblox game ideas for new developers. ".repeat(10),
            tags: vec!["roblox game ideas".into(), "roblox".into(), "game dev".into(), "roblox studio".into(), "ideas".into()],
            keyword: Some("roblox game ideas".into()),
        };
        let report = score(&input, &["roblox".into(), "roblox studio".into()]);
        assert!(report.score >= 70, "score {}", report.score);
        let empty = score(&SeoInput { title: "x".into(), description: String::new(), tags: vec![], keyword: Some("roblox".into()) }, &[]);
        assert!(empty.score < 10);
    }

    #[test]
    fn flags_controversial_words() {
        assert_eq!(controversial_words("I KILLED the boss, free robux!"), vec!["free robux", "killed"]);
        assert!(controversial_words("skilled builder").is_empty());
    }
}
