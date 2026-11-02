//! Reads the in-game citizen list from the words Textract found on a screenshot.
//!
//! The `Déf.` column shows the value the nightly attack compares against, so it is used as read.
//! Layout of a row (`templates/ajax/game/town/citizen.html.twig` in the game's source):
//! avatar (often with text in it), job icon, name, home message, `N pts` or `--`, location.
//! Rows are found through the name column rather than through `pts`: Textract doesn't always see
//! a `--`, and that citizen must not disappear from the list.

use crate::config::SimulationCitizen;
use std::collections::HashSet;

/// Below this confidence (in %), a name or defense is pointed out in the review message.
const LOW_CONFIDENCE: f32 = 90.0;
/// Width of the window looking for the name column, as a fraction of the image width.
const NAME_COLUMN_WIDTH: f32 = 0.01;
/// How far a defense may start from its column, as a fraction of the image width.
const DEFENSE_COLUMN_TOLERANCE: f32 = 0.03;
/// Below this fraction of the row height, two words of the name column are the same row (a home
/// message wrapped under the name).
const SAME_ROW_FRACTION: f32 = 0.7;
/// The message must stay well within Discord's 2000 characters.
const MAX_REVIEW_MESSAGE_LENGTH: usize = 1900;

/// Textract's reference gives 5 MB as the limit for documents sent as bytes.
pub const MAX_CAPTURE_BYTES: u64 = 5 * 1024 * 1024;

/// `custom_id` of the file upload in the `/debordo-complete` form.
pub const CAPTURE_INPUT: &str = "capture_input";

/// `custom_id` of the "Simuler" button of the review message.
pub const REVIEW_CITIZENS_BUTTON: &str = "review_citizens";

/// First words of the "Mysterious Stranger" row, followed by the second ones.
const STRANGER_NAMES: [(&str, &str); 4] = [
    ("mystérieux", "étranger"),
    ("mysterious", "stranger"),
    ("mysteriöser", "fremder"),
    ("extraño", "misterioso"),
];

/// Footer labels following the number of living citizens.
const FOOTER_LABELS: [&str; 4] = ["citoyens", "citizens", "habitantes", "bürger"];

/// A word found on the image. Positions are fractions of the image size.
#[derive(Debug, Clone, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
    pub confidence: f32,
}

impl OcrWord {
    fn center_y(&self) -> f32 {
        self.top + self.height / 2.0
    }
}

/// A living citizen read on the image; `defense` is `None` when the cell looked empty.
#[derive(Debug, Clone, PartialEq)]
pub struct CitizenRow {
    pub name: String,
    pub defense: Option<i32>,
    pub low_confidence: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedCapture {
    pub rows: Vec<CitizenRow>,
    /// Living citizens according to the footer, when the capture includes it.
    pub footer_count: Option<usize>,
}

/// The citizens to simulate, with what the user should double-check.
#[derive(Debug, Clone, Default)]
pub struct ReadCitizens {
    pub citizens: Vec<SimulationCitizen>,
    pub warnings: Vec<String>,
}

fn median(mut values: Vec<f32>) -> Option<f32> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f32::total_cmp);
    Some(values[values.len() / 2])
}

fn is_pts(text: &str) -> bool {
    text.trim_end_matches('.').eq_ignore_ascii_case("pts")
}

/// The defense value in `word` when it is followed by `pts` in the same row, or written as `12pts`.
fn defense_token(word: &OcrWord, words: &[OcrWord], row_height: f32) -> Option<i32> {
    let text = word.text.trim_end_matches('.');
    if let Some(digits) = text
        .strip_suffix("pts")
        .or_else(|| text.strip_suffix("PTS"))
        .filter(|d| !d.is_empty())
    {
        return digits.parse().ok();
    }
    let value = word.text.parse().ok()?;
    let word_right = word.left + word.width;
    words
        .iter()
        .any(|w| {
            is_pts(&w.text)
                && (w.center_y() - word.center_y()).abs() < row_height / 2.0
                && w.left >= word_right - 0.005
                && w.left - word_right < 0.05
        })
        .then_some(value)
}

/// Left edge of the column holding the most words left of `limit`.
fn name_column(words: &[OcrWord], limit: f32) -> Option<f32> {
    let mut lefts: Vec<f32> = words
        .iter()
        .filter(|w| w.left < limit)
        .map(|w| w.left)
        .collect();
    lefts.sort_by(f32::total_cmp);
    let mut best: Option<(usize, f32)> = None;
    for (start, &left) in lefts.iter().enumerate() {
        let count = lefts[start..]
            .iter()
            .take_while(|&&l| l - left <= NAME_COLUMN_WIDTH)
            .count();
        if best.is_none_or(|(most, _)| count > most) {
            best = Some((count, left));
        }
    }
    best.map(|(_, left)| left)
}

fn clean_name(text: &str) -> String {
    text.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
        .to_string()
}

/// A provisional row height, from the words that look like a defense or like `pts`.
fn estimate_row_height(words: &[OcrWord]) -> f32 {
    let mut tops: Vec<f32> = words
        .iter()
        .filter(|w| is_pts(&w.text) || w.text.to_ascii_lowercase().ends_with("pts"))
        .map(OcrWord::center_y)
        .collect();
    tops.sort_by(f32::total_cmp);
    let gaps: Vec<f32> = tops.windows(2).map(|p| p[1] - p[0]).collect();
    median(gaps).unwrap_or(0.05)
}

/// Finds the living citizens among the words of a screenshot of the citizen list.
#[must_use]
pub fn parse_citizen_rows(words: &[OcrWord]) -> ParsedCapture {
    let provisional_height = estimate_row_height(words);
    let tokens: Vec<(&OcrWord, i32)> = words
        .iter()
        .filter_map(|w| defense_token(w, words, provisional_height).map(|value| (w, value)))
        .collect();
    let Some(defense_left) = median(tokens.iter().map(|(w, _)| w.left).collect()) else {
        return ParsedCapture::default();
    };
    let Some(name_left) = name_column(words, defense_left) else {
        return ParsedCapture::default();
    };

    let mut names: Vec<&OcrWord> = words
        .iter()
        .filter(|w| (w.left - name_left).abs() <= NAME_COLUMN_WIDTH)
        .collect();
    names.sort_by(|a, b| a.center_y().total_cmp(&b.center_y()));

    let gaps: Vec<f32> = names
        .windows(2)
        .map(|p| p[1].center_y() - p[0].center_y())
        .collect();
    let row_height = median(gaps).unwrap_or(provisional_height);

    // A wrapped home message starts at the name column too: only the top word of a row is a name.
    let mut row_names: Vec<&OcrWord> = Vec::new();
    for name in names {
        let same_row = row_names
            .last()
            .is_some_and(|prev| name.center_y() - prev.center_y() < row_height * SAME_ROW_FRACTION);
        if !same_row {
            row_names.push(name);
        }
    }

    let location_left = defense_left + (1.0 - defense_left) / 2.0;
    let in_row = |word: &OcrWord, name: &OcrWord| {
        (word.center_y() - name.center_y()).abs() < row_height / 2.0
    };

    let mut rows = Vec::new();
    for name in &row_names {
        let is_stranger = STRANGER_NAMES.iter().any(|(first, second)| {
            name.text.to_lowercase() == *first
                && words
                    .iter()
                    .any(|w| in_row(w, name) && w.text.to_lowercase() == *second)
        });
        if is_stranger {
            continue;
        }

        let token = tokens
            .iter()
            .filter(|(w, _)| {
                in_row(w, name) && (w.left - defense_left).abs() <= DEFENSE_COLUMN_TOLERANCE
            })
            .min_by(|(a, _), (b, _)| {
                let distance = |w: &OcrWord| (w.center_y() - name.center_y()).abs();
                distance(a).total_cmp(&distance(b))
            });
        let dash = words.iter().any(|w| {
            w.text == "--"
                && in_row(w, name)
                && (w.left - defense_left).abs() <= DEFENSE_COLUMN_TOLERANCE
        });
        // Dead citizens have no defense cell nor location: the far right of their row stays empty.
        let has_location = words
            .iter()
            .any(|w| in_row(w, name) && w.left >= location_left);
        if token.is_none() && !dash && !has_location {
            continue;
        }

        let cleaned = clean_name(&name.text);
        if cleaned.is_empty() {
            continue;
        }
        let token_confidence = token.map_or(100.0, |(w, _)| w.confidence);
        rows.push(CitizenRow {
            name: cleaned,
            defense: token.map(|(_, value)| *value).or(dash.then_some(0)),
            low_confidence: name.confidence.min(token_confidence) < LOW_CONFIDENCE,
        });
    }

    let footer_count = words.iter().find_map(|w| {
        let count = w.text.parse::<usize>().ok()?;
        words
            .iter()
            .any(|label| {
                let text = label.text.to_lowercase();
                FOOTER_LABELS.contains(&text.as_str())
                    && label.left > w.left
                    && label.left - w.left < 0.1
                    && (label.center_y() - w.center_y()).abs() < row_height / 2.0
            })
            .then_some(count)
    });

    ParsedCapture { rows, footer_count }
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != cb);
            current.push(substitution.min(previous[j + 1] + 1).min(current[j] + 1));
        }
        previous = current;
    }
    previous[b.len()]
}

fn name_list(names: &[String]) -> String {
    names.join(", ")
}

fn low_confidence_warning(rows: &[&CitizenRow]) -> Option<String> {
    let names: Vec<String> = rows
        .iter()
        .filter(|r| r.low_confidence)
        .map(|r| r.name.clone())
        .collect();
    (!names.is_empty()).then(|| format!("Lecture incertaine : {}.", name_list(&names)))
}

/// Citizens from the capture alone: every row counts, an empty defense cell being `--` (0).
#[must_use]
pub fn citizens_from_rows(capture: &ParsedCapture) -> ReadCitizens {
    let mut seen = HashSet::new();
    let rows: Vec<&CitizenRow> = capture
        .rows
        .iter()
        .filter(|r| seen.insert(r.name.to_lowercase()))
        .collect();

    let mut warnings = Vec::new();
    let empty: Vec<String> = rows
        .iter()
        .filter(|r| r.defense.is_none())
        .map(|r| r.name.clone())
        .collect();
    if !empty.is_empty() {
        warnings.push(format!(
            "Défense non lue, comptée à 0 : {}.",
            name_list(&empty)
        ));
    }
    warnings.extend(low_confidence_warning(&rows));
    if let Some(count) = capture.footer_count
        && count != rows.len()
    {
        warnings.push(format!(
            "{} citoyens lus, mais la capture en annonce {count}.",
            rows.len()
        ));
    }

    ReadCitizens {
        citizens: rows
            .iter()
            .map(|r| SimulationCitizen {
                name: r.name.clone(),
                defense: r.defense.unwrap_or(0),
            })
            .collect(),
        warnings,
    }
}

/// The row of `name`: same name first, else the only row close enough (a misread name).
fn matching_row(name: &str, rows: &[CitizenRow], taken: &[bool]) -> Option<usize> {
    let lower = name.to_lowercase();
    let free = || rows.iter().enumerate().filter(|(i, _)| !taken[*i]);
    if let Some((i, _)) = free().find(|(_, r)| r.name.to_lowercase() == lower) {
        return Some(i);
    }
    // Same tolerance as the game's own name checks: short names are too close to each other.
    let tolerance = if name.chars().count() <= 5 { 1 } else { 2 };
    let mut close =
        free().filter(|(_, r)| levenshtein(&r.name.to_lowercase(), &lower) <= tolerance);
    let first = close.next()?;
    close.next().is_none().then_some(first.0)
}

/// Citizens from the API's list, with the defenses read on the capture. The API gives the exact
/// names and only the living citizens; its estimates stand in for what the capture doesn't show.
#[must_use]
pub fn reconcile_with_api(
    capture: &ParsedCapture,
    api_citizens: &[SimulationCitizen],
) -> ReadCitizens {
    let mut taken = vec![false; capture.rows.len()];
    let mut citizens = Vec::with_capacity(api_citizens.len());
    let mut not_read = Vec::new();
    let mut matched = Vec::new();

    // Exact names first, so a close name can't take the row of the citizen it belongs to.
    let mut assignments = vec![None; api_citizens.len()];
    for (slot, citizen) in assignments.iter_mut().zip(api_citizens) {
        let lower = citizen.name.to_lowercase();
        if let Some(i) = capture
            .rows
            .iter()
            .enumerate()
            .position(|(i, r)| !taken[i] && r.name.to_lowercase() == lower)
        {
            taken[i] = true;
            *slot = Some(i);
        }
    }
    for (slot, citizen) in assignments.iter_mut().zip(api_citizens) {
        if slot.is_none()
            && let Some(i) = matching_row(&citizen.name, &capture.rows, &taken)
        {
            taken[i] = true;
            *slot = Some(i);
        }
    }

    for (slot, citizen) in assignments.iter().zip(api_citizens) {
        let row = slot.map(|i| &capture.rows[i]);
        if let Some(row) = row {
            matched.push(row);
        }
        match row.and_then(|r| r.defense) {
            Some(defense) => citizens.push(SimulationCitizen {
                name: citizen.name.clone(),
                defense,
            }),
            None => {
                // An estimate of 0 is what an unread `--` means: nothing to point out.
                if citizen.defense != 0 || row.is_none() {
                    not_read.push(citizen.name.clone());
                }
                citizens.push(citizen.clone());
            }
        }
    }

    let mut warnings = Vec::new();
    if !not_read.is_empty() {
        warnings.push(format!(
            "Défense non lue, estimée avec l'API : {}.",
            name_list(&not_read)
        ));
    }
    warnings.extend(low_confidence_warning(&matched));
    let unknown: Vec<String> = capture
        .rows
        .iter()
        .zip(&taken)
        .filter(|(_, taken)| !**taken)
        .map(|(r, _)| r.name.clone())
        .collect();
    if !unknown.is_empty() {
        warnings.push(format!(
            "Absents de l'API, ignorés : {}.",
            name_list(&unknown)
        ));
    }

    ReadCitizens { citizens, warnings }
}

/// The review message, or `None` if the configuration itself wouldn't fit. Warnings are the ones
/// to go when space runs out: the code block is what the "Simuler" button reads back.
#[must_use]
pub fn review_message(citizen_count: usize, conf: &str, warnings: &[String]) -> Option<String> {
    let head = format!("📷 **{citizen_count} citoyens lus sur la capture.**\n");
    let tail = format!("```\n{conf}\n```\nVérifiez les valeurs, puis cliquez sur **Simuler**.");
    let mut available = MAX_REVIEW_MESSAGE_LENGTH.checked_sub(head.len() + tail.len())?;

    let mut message = head;
    for warning in warnings {
        let line = format!("⚠️ {warning}\n");
        if line.len() > available {
            let omitted = "⚠️ Autres avertissements omis.\n";
            if omitted.len() <= available {
                message.push_str(omitted);
            }
            break;
        }
        available -= line.len();
        message.push_str(&line);
    }
    message.push_str(&tail);
    Some(message)
}

/// The configuration in the code block of a review message.
#[must_use]
pub fn review_message_conf(content: &str) -> Option<&str> {
    let (_, rest) = content.split_once("```\n")?;
    let (conf, _) = rest.split_once("\n```")?;
    Some(conf)
}

/// Whether the worker may download `url`: the interaction is signed, but it fetches this address
/// with its own network access, so only Discord's CDN is accepted.
#[must_use]
pub fn is_discord_cdn_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    matches!(host, "cdn.discordapp.com" | "media.discordapp.net")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Textract's output for a real capture (`E:\code\demo-mh.png`), reduced to its WORD blocks:
    /// a town where all the heroes have 2 defense and the two non-heroes show `--`.
    const REAL_CAPTURE: &str = include_str!("../tests/data/citizens_textract_words.json");

    fn real_words() -> Vec<OcrWord> {
        let json: serde_json::Value = serde_json::from_str(REAL_CAPTURE).unwrap();
        json["Blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                let g = &b["Geometry"]["BoundingBox"];
                let number = |key: &str| g[key].as_f64().unwrap() as f32;
                OcrWord {
                    text: b["Text"].as_str().unwrap().to_string(),
                    left: number("Left"),
                    top: number("Top"),
                    width: number("Width"),
                    height: number("Height"),
                    confidence: b["Confidence"].as_f64().unwrap() as f32,
                }
            })
            .collect()
    }

    fn word(text: &str, left: f32, row: f32) -> OcrWord {
        OcrWord {
            text: text.to_string(),
            left,
            top: 0.02 + row * 0.05,
            width: 0.01 * text.chars().count() as f32,
            height: 0.01,
            confidence: 99.0,
        }
    }

    const NAME: f32 = 0.2;
    const DEFENSE: f32 = 0.7;
    const LOCATION: f32 = 0.95;

    /// A row with a name, `value pts` and a location.
    fn row(name: &str, value: &str, index: f32) -> Vec<OcrWord> {
        vec![
            word(name, NAME, index),
            word(value, DEFENSE, index),
            word("pts", DEFENSE + 0.012 * value.chars().count() as f32, index),
            word("--", LOCATION, index),
        ]
    }

    fn rows(words: &[OcrWord]) -> Vec<(String, Option<i32>)> {
        parse_citizen_rows(words)
            .rows
            .into_iter()
            .map(|r| (r.name, r.defense))
            .collect()
    }

    fn citizen(name: &str, defense: i32) -> SimulationCitizen {
        SimulationCitizen {
            name: name.to_string(),
            defense,
        }
    }

    #[test]
    fn reads_the_real_capture() {
        let capture = parse_citizen_rows(&real_words());
        let names: Vec<&str> = capture.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "AJO",
                "ApprentiBiers",
                "Axfalt",
                "BadJoker",
                "BadRedMan",
                "bargou",
                "Bouboulpe",
                "Brimouille",
                "cloute",
                "Crab",
                "Csag",
                "Cubique",
                "Dinoz",
                "HuangZhang",
                "ilipika",
                "Jolapoule",
                "KermitADC",
                "Klershwagen",
                "LoupNoir",
                "Nicoof07",
                "Nikeu01",
                "oam",
                "Perfect0",
                "Personne",
                "Reivax",
                "Rigorda",
                "Sanom",
                "Schulenina",
                "Scipion",
                "Skaric",
                "SnakePlissken",
                "Snow",
                "Spongebob",
                "Suri",
                "Tala",
                "Vilmarh",
                "vinnou",
                "Walui",
                "Waset",
                "Winston",
            ]
        );
        for row in &capture.rows {
            let expected = if matches!(row.name.as_str(), "cloute" | "Csag") {
                None
            } else {
                Some(2)
            };
            assert_eq!(row.defense, expected, "{}", row.name);
        }
        assert_eq!(capture.footer_count, None, "this capture has no footer");

        let read = citizens_from_rows(&capture);
        assert_eq!(read.citizens.len(), 40);
        assert_eq!(
            read.warnings,
            ["Défense non lue, comptée à 0 : cloute, Csag."]
        );
    }

    #[test]
    fn reads_several_digits() {
        let mut words = row("Alice", "7", 0.0);
        words.extend(row("Bob", "28", 1.0));
        words.extend(row("Carol", "143", 2.0));
        assert_eq!(
            rows(&words),
            [
                ("Alice".to_string(), Some(7)),
                ("Bob".to_string(), Some(28)),
                ("Carol".to_string(), Some(143)),
            ]
        );
    }

    #[test]
    fn reads_a_value_stuck_to_pts() {
        let mut words = vec![word("Alice", NAME, 0.0), word("12pts", DEFENSE, 0.0)];
        words.extend(row("Bob", "3", 1.0));
        words.extend(row("Carol", "4", 2.0));
        assert_eq!(rows(&words)[0], ("Alice".to_string(), Some(12)));
    }

    #[test]
    fn ignores_avatar_text_job_icons_and_home_messages() {
        let mut words = Vec::new();
        for (i, name) in ["Alice", "Bob", "Carol"].iter().enumerate() {
            let index = i as f32;
            words.extend(row(name, "5", index));
            // As on a real capture: only some of these are found, and they are not aligned.
            words.push(word("AVATAR", 0.03 + index * 0.04, index));
            if i == 1 {
                words.push(word("D-D", 0.17, index));
                words.push(word("Vol", 0.35, index));
                words.push(word("agression", 0.4, index));
            }
        }
        let names: Vec<String> = rows(&words).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn a_wrapped_home_message_is_not_a_citizen() {
        let mut words = row("Alice", "5", 0.0);
        words.push(OcrWord {
            top: 0.02 + 0.012,
            ..word("message", NAME, 0.0)
        });
        words.extend(row("Bob", "6", 1.0));
        words.extend(row("Carol", "7", 2.0));
        let names: Vec<String> = rows(&words).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["Alice", "Bob", "Carol"]);
    }

    #[test]
    fn a_dash_only_counts_in_the_defense_column() {
        let mut words = row("Alice", "5", 0.0);
        words.extend(row("Bob", "6", 1.0));
        words.extend([
            word("Carol", NAME, 2.0),
            word("--", DEFENSE, 2.0),
            word("--", LOCATION, 2.0),
        ]);
        words.extend([word("Dave", NAME, 3.0), word("--", LOCATION, 3.0)]);
        assert_eq!(
            rows(&words),
            [
                ("Alice".to_string(), Some(5)),
                ("Bob".to_string(), Some(6)),
                ("Carol".to_string(), Some(0)),
                ("Dave".to_string(), None),
            ]
        );
    }

    #[test]
    fn skips_dead_citizens() {
        let mut words = row("Alice", "5", 0.0);
        words.extend(row("Bob", "6", 1.0));
        words.extend([
            word("Carol", NAME, 2.0),
            word("--", NAME + 0.15, 2.0),
            word("Coffre", DEFENSE - 0.05, 2.0),
            word("vide", DEFENSE + 0.03, 2.0),
        ]);
        let names: Vec<String> = rows(&words).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["Alice", "Bob"]);
    }

    #[test]
    fn skips_the_stranger() {
        let mut words = row("Alice", "5", 0.0);
        words.extend(row("Bob", "6", 1.0));
        words.extend([
            word("Mystérieux", NAME, 2.0),
            word("étranger", NAME + 0.12, 2.0),
            word("--", DEFENSE, 2.0),
            word("--", LOCATION, 2.0),
        ]);
        let names: Vec<String> = rows(&words).into_iter().map(|r| r.0).collect();
        assert_eq!(names, ["Alice", "Bob"]);
    }

    #[test]
    fn an_unrelated_image_gives_no_row() {
        let words = [word("Bonjour", 0.2, 0.0), word("monde", 0.3, 0.0)];
        assert_eq!(parse_citizen_rows(&words), ParsedCapture::default());
    }

    #[test]
    fn warns_when_the_footer_disagrees() {
        let mut words = row("Alice", "5", 0.0);
        words.extend(row("Bob", "6", 1.0));
        words.extend([word("38", 0.4, 3.0), word("citoyens", 0.43, 3.0)]);
        let capture = parse_citizen_rows(&words);
        assert_eq!(capture.footer_count, Some(38));
        assert_eq!(
            citizens_from_rows(&capture).warnings,
            ["2 citoyens lus, mais la capture en annonce 38."]
        );
    }

    #[test]
    fn points_out_low_confidence() {
        let mut words = row("Alice", "5", 0.0);
        words[1].confidence = 60.0;
        words.extend(row("Bob", "6", 1.0));
        let read = citizens_from_rows(&parse_citizen_rows(&words));
        assert_eq!(read.warnings, ["Lecture incertaine : Alice."]);
    }

    fn capture_of(rows: &[(&str, Option<i32>)]) -> ParsedCapture {
        ParsedCapture {
            rows: rows
                .iter()
                .map(|(name, defense)| CitizenRow {
                    name: (*name).to_string(),
                    defense: *defense,
                    low_confidence: false,
                })
                .collect(),
            footer_count: None,
        }
    }

    #[test]
    fn reconcile_matches_exact_names_ignoring_case() {
        let read = reconcile_with_api(&capture_of(&[("ALICE", Some(9))]), &[citizen("Alice", 1)]);
        assert_eq!(read.citizens[0].name, "Alice");
        assert_eq!(read.citizens[0].defense, 9);
        assert!(read.warnings.is_empty());
    }

    #[test]
    fn reconcile_corrects_a_misread_name() {
        let read = reconcile_with_api(
            &capture_of(&[("Klershwagem", Some(9))]),
            &[citizen("Klershwagen", 1)],
        );
        assert_eq!(read.citizens[0].name, "Klershwagen");
        assert_eq!(read.citizens[0].defense, 9);
        assert!(read.warnings.is_empty());
    }

    #[test]
    fn reconcile_prefers_the_exact_name_over_a_close_one() {
        let read = reconcile_with_api(
            &capture_of(&[("Nikeu01", Some(9)), ("Nikeu02", Some(4))]),
            &[citizen("Nikeu02", 1), citizen("Nikeu01", 1)],
        );
        assert_eq!(read.citizens[0].defense, 4);
        assert_eq!(read.citizens[1].defense, 9);
    }

    #[test]
    fn reconcile_refuses_an_ambiguous_name() {
        let read = reconcile_with_api(
            &capture_of(&[("Nikeu01", Some(9)), ("Nikeu02", Some(4))]),
            &[citizen("Nikeu03", 1)],
        );
        assert_eq!(read.citizens[0].defense, 1);
        assert_eq!(read.warnings.len(), 2);
    }

    #[test]
    fn reconcile_is_stricter_on_short_names() {
        let read = reconcile_with_api(&capture_of(&[("oxm", Some(9))]), &[citizen("oam", 1)]);
        assert_eq!(read.citizens[0].defense, 9);
        let read = reconcile_with_api(&capture_of(&[("oxx", Some(9))]), &[citizen("oam", 1)]);
        assert_eq!(read.citizens[0].defense, 1);
    }

    #[test]
    fn reconcile_keeps_the_estimate_of_missing_citizens() {
        let read = reconcile_with_api(&capture_of(&[]), &[citizen("Alice", 7)]);
        assert_eq!(read.citizens[0].defense, 7);
        assert_eq!(
            read.warnings,
            ["Défense non lue, estimée avec l'API : Alice."]
        );
    }

    #[test]
    fn reconcile_is_silent_about_an_unread_dash_estimated_at_zero() {
        let read = reconcile_with_api(&capture_of(&[("cloute", None)]), &[citizen("cloute", 0)]);
        assert_eq!(read.citizens[0].defense, 0);
        assert!(read.warnings.is_empty());
    }

    #[test]
    fn reconcile_drops_rows_the_api_does_not_know() {
        let read = reconcile_with_api(
            &capture_of(&[("Alice", Some(3)), ("Zzzzzz", Some(4))]),
            &[citizen("Alice", 1)],
        );
        assert_eq!(read.citizens.len(), 1);
        assert_eq!(read.warnings, ["Absents de l'API, ignorés : Zzzzzz."]);
    }

    #[test]
    fn review_message_round_trips_its_configuration() {
        let conf = "defense: 100\ntdg: 10-20\n---\nAlice: 3";
        let message = review_message(1, conf, &["Attention.".to_string()]).unwrap();
        assert_eq!(review_message_conf(&message), Some(conf));
        assert!(message.contains("⚠️ Attention."));
    }

    #[test]
    fn review_message_drops_warnings_rather_than_the_configuration() {
        let conf = "x".repeat(1500);
        let warnings = vec!["w".repeat(300), "w".repeat(300)];
        let message = review_message(40, &conf, &warnings).unwrap();
        assert!(message.chars().count() <= MAX_REVIEW_MESSAGE_LENGTH);
        assert_eq!(review_message_conf(&message), Some(conf.as_str()));
        assert!(message.contains("omis"));
    }

    #[test]
    fn review_message_refuses_a_configuration_too_long() {
        assert_eq!(review_message(40, &"x".repeat(2000), &[]), None);
    }

    #[test]
    fn only_accepts_discord_cdn_urls() {
        assert!(is_discord_cdn_url(
            "https://cdn.discordapp.com/ephemeral-attachments/1/2/a.png?ex=1"
        ));
        assert!(is_discord_cdn_url("https://media.discordapp.net/a.png"));
        assert!(!is_discord_cdn_url("http://cdn.discordapp.com/a.png"));
        assert!(!is_discord_cdn_url(
            "https://cdn.discordapp.com.evil.com/a.png"
        ));
        assert!(!is_discord_cdn_url(
            "https://evil.com/cdn.discordapp.com/a.png"
        ));
        assert!(!is_discord_cdn_url(
            "https://cdn.discordapp.com@evil.com/a.png"
        ));
    }
}
