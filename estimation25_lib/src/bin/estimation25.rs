//! Local CLI: `estimation25 [--api [--userkey K]] [--town_id ID] [--jour N] [--demain] [--mode M]
//! [--seeds A-B] [--ames N] [--ames-veille N] [--penalite P] [--ames-max M] [FICHIER]`.
//!
//! Readings come from one of:
//! - `--town_id ID --jour N`: `MyHordes` Optimizer, no `MyHordes` key needed;
//! - `--api`: `MyHordes` Optimizer, the town and day coming from the `MyHordes` API (user key from
//!   `--userkey` or `MH_USER_KEY`, application key from `MH_APP_KEY`), either overridable;
//! - otherwise FICHIER, or stdin when omitted.
//!
//! The 2^32 PHP seeds are then replayed to find the ones reproducing every reading, and the attack
//! range they imply is printed.

use estimation25_lib::mho::{self, MhoEstimations};
use estimation25_lib::parse::{InputOverrides, parse_bool, parse_mode};
use estimation25_lib::{
    EstimConf, Estimate, EstimationInput, estimate, format_summary, parse_text,
};
use serde::Deserialize;
use std::fmt::Display;
use std::io::Read;
use std::ops::RangeInclusive;
use std::process::ExitCode;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

// `concat!` keeps the indentation (a trailing `\` in a string literal would strip it).
const USAGE: &str = concat!(
    "Usage : estimation25 [--api [--userkey CLÉ]] [--town_id ID] [--jour N] [--demain] ",
    "[--mode normal|hard|easy] [--seeds A-B] [--ames N] [--ames-veille N] [--penalite 0.04] ",
    "[--ames-max 1.2] [RELEVÉS]\n",
    "Source des relevés :\n",
    "  --town_id ID --jour N  MyHordes Optimizer, sans clé MyHordes ",
    "(Pandémonium : ajoutez --ames-max 666)\n",
    "  --api                  MyHordes Optimizer, ville et jour lus via l'API MyHordes\n",
    "                         (clé utilisateur : --userkey ou MH_USER_KEY, ",
    "clé d'application : MH_APP_KEY)\n",
    "  sinon                  RELEVÉS, ou l'entrée standard ",
    "(une ligne `33% 2047 - 2749` par citoyen)",
);

const MH_ME_URL: &str = "https://myhordes.eu/api/x/json/me";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
/// `modifiers.red_soul_max_factor` of Pandemonium towns.
const PANDEMONIUM_SOUL_MAX: f64 = 666.0;

struct Args {
    overrides: InputOverrides,
    file: Option<String>,
    seeds: RangeInclusive<u32>,
    api: bool,
    user_key: Option<String>,
    town_id: Option<i64>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            overrides: InputOverrides::default(),
            file: None,
            seeds: 0..=u32::MAX,
            api: false,
            user_key: None,
            town_id: None,
        }
    }
}

/// Parses an option value, naming the option in the error.
fn parse_value<T: FromStr>(option: &str, raw: &str) -> Result<T, String>
where
    T::Err: Display,
{
    raw.trim().parse().map_err(|e| format!("{option} : {e}"))
}

fn parse_seed_range(raw: &str) -> Result<RangeInclusive<u32>, String> {
    let (a, b) = raw
        .split_once('-')
        .ok_or_else(|| format!("--seeds attend A-B : {raw}"))?;
    Ok(parse_value("--seeds", a)?..=parse_value("--seeds", b)?)
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} attend une valeur"));
        let o = &mut args.overrides;
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--jour" | "--day" => {
                let v = value("--jour")?;
                o.day = Some(parse_value("--jour", v.trim_start_matches(['J', 'j']))?);
            }
            "--demain" | "--j1" => o.future = Some(true),
            "--mode" => {
                let v = value("--mode")?;
                o.mode = Some(parse_mode(&v).ok_or_else(|| format!("mode inconnu : {v}"))?);
            }
            "--ames" => o.red_souls = Some(parse_value("--ames", &value("--ames")?)?),
            "--ames-veille" => {
                o.planner_red_souls = Some(parse_value("--ames-veille", &value("--ames-veille")?)?);
            }
            "--penalite" => {
                let v = value("--penalite")?.replace(',', ".");
                o.soul_penalty = Some(parse_value("--penalite", &v)?);
            }
            "--ames-max" => o.soul_max = Some(parse_value("--ames-max", &value("--ames-max")?)?),
            "--seeds" => args.seeds = parse_seed_range(&value("--seeds")?)?,
            "--api" => args.api = true,
            "--userkey" => args.user_key = Some(value("--userkey")?),
            "--town_id" | "--town-id" => {
                args.town_id = Some(parse_value("--town_id", &value("--town_id")?)?);
            }
            s if s.starts_with("--demain=") => {
                let v = &s["--demain=".len()..];
                o.future = Some(parse_bool(v).ok_or_else(|| format!("valeur invalide : {s}"))?);
            }
            s if s.starts_with('-') => return Err(format!("option inconnue : {s}\n{USAGE}")),
            s => args.file = Some(s.to_string()),
        }
    }
    Ok(args)
}

/// Seed search with a progress line on stderr; returns the estimate and a footer.
fn run_search(
    input: &EstimationInput,
    seeds: RangeInclusive<u32>,
) -> Result<(Estimate, String), String> {
    let total = u64::from(*seeds.end()) - u64::from(*seeds.start()) + 1;
    let progress = AtomicU64::new(0);
    let done = AtomicBool::new(false);
    let result = std::thread::scope(|scope| {
        scope.spawn(|| {
            let start = Instant::now();
            while !done.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(500));
                let p = progress.load(Ordering::Relaxed) as f64 / total as f64;
                let secs = start.elapsed().as_secs_f64();
                let eta = if p > 0.0 { secs / p - secs } else { 0.0 };
                eprint!(
                    "\rseeds : {:5.1} % ({secs:.0} s, reste ~{eta:.0} s)   ",
                    p * 100.0
                );
            }
            eprintln!();
        });
        let result = estimate(input, &EstimConf::default(), seeds, &progress);
        done.store(true, Ordering::Relaxed);
        result
    });
    let estimate = result.map_err(|e| format!("Erreur : {e}"))?;
    Ok((estimate, format!("{total} seeds testées")))
}

fn read_readings(file: Option<&str>) -> Result<String, String> {
    if let Some(path) = file {
        return std::fs::read_to_string(path).map_err(|e| format!("lecture de {path} : {e}"));
    }
    eprintln!("Collez les relevés puis Ctrl-Z/Entrée (Windows) ou Ctrl-D :");
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Town, current day and Pandemonium flag of the user, from the `MyHordes` API.
struct Town {
    id: i64,
    day: i64,
    pandemonium: bool,
}

fn get(request: reqwest::blocking::RequestBuilder, what: &str) -> Result<String, String> {
    let response = request
        .send()
        .map_err(|e| format!("{what} injoignable : {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .map_err(|e| format!("{what} : réponse illisible : {e}"))?;
    if status.is_success() {
        Ok(body)
    } else {
        Err(format!("{what} a répondu {status} : {body}"))
    }
}

fn fetch_town(client: &reqwest::blocking::Client, user_key: &str) -> Result<Town, String> {
    #[derive(Deserialize)]
    struct Me {
        map: Option<Map>,
    }
    #[derive(Deserialize)]
    struct Map {
        id: i64,
        days: i64,
        city: Option<City>,
    }
    #[derive(Deserialize)]
    struct City {
        #[serde(default)]
        hard: bool,
    }

    let app_key = std::env::var("MH_APP_KEY")
        .map_err(|_| "--api : clé d'application manquante (variable MH_APP_KEY)".to_string())?;
    let request = client.get(MH_ME_URL).query(&[
        ("userkey", user_key),
        ("appkey", app_key.as_str()),
        ("fields", "map.fields(id,days,city.fields(hard))"),
    ]);
    let body = get(request, "l'API MyHordes")?;
    let me: Me =
        serde_json::from_str(&body).map_err(|e| format!("réponse MyHordes inattendue : {e}"))?;
    let map = me
        .map
        .ok_or("l'API MyHordes ne renvoie aucune ville : êtes-vous incarné ?")?;
    Ok(Town {
        id: map.id,
        day: map.days,
        pandemonium: map.city.is_some_and(|c| c.hard),
    })
}

/// What the watchtower showed on `day` according to `MyHordes` Optimizer (`None` before day 1).
fn fetch_estimations(
    client: &reqwest::blocking::Client,
    day: i64,
    town_id: i64,
) -> Result<Option<MhoEstimations>, String> {
    if day < 1 {
        return Ok(None);
    }
    let (header, origin) = mho::ORIGIN_HEADER;
    let request = client
        .get(mho::estimations_url(day, town_id))
        .header(header, origin);
    let body = get(request, "MyHordes Optimizer")?;
    MhoEstimations::from_json(&body)
        .map(Some)
        .map_err(|e| format!("réponse MHO inattendue : {e}"))
}

/// `--api`: readings of the attack from `MyHordes` Optimizer.
fn api_input(args: &Args) -> Result<EstimationInput, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;

    let town = match (args.town_id, args.overrides.day) {
        (Some(id), Some(day)) => Town {
            id,
            day,
            pandemonium: false,
        },
        (town_override, day_override) => {
            let user_key = args
                .user_key
                .clone()
                .or_else(|| std::env::var("MH_USER_KEY").ok())
                .ok_or(
                    "clé utilisateur MyHordes manquante (--userkey ou MH_USER_KEY) ; \
                     sans clé, précisez --town_id ID et --jour N",
                )?;
            let town = fetch_town(&client, &user_key)?;
            Town {
                id: town_override.unwrap_or(town.id),
                day: day_override.unwrap_or(town.day),
                pandemonium: town.pandemonium,
            }
        }
    };

    // The attack's readings are `estim` of its day and `planif` (J+1) of the day before.
    let attack_day = town.day + i64::from(args.overrides.future.unwrap_or(false));
    let [attack_payload, eve_payload] = mho::payload_days(attack_day);
    let attack = fetch_estimations(&client, attack_payload, town.id)?;
    let eve = fetch_estimations(&client, eve_payload, town.id)?;
    eprintln!(
        "MHO : ville {}, attaque du J{attack_day} : {} relevés du J{attack_payload}, {} relevés J+1 du J{eve_payload}{}",
        town.id,
        attack.as_ref().map_or(0, |e| e.today().len()),
        eve.as_ref().map_or(0, |e| e.planner().len()),
        if town.pandemonium {
            " (Pandémonium)"
        } else {
            ""
        }
    );

    let mut overrides = args.overrides.clone();
    if town.pandemonium && overrides.soul_max.is_none() {
        overrides.soul_max = Some(PANDEMONIUM_SOUL_MAX);
    }
    mho::attack_input(attack_day, attack.as_ref(), eve.as_ref(), &overrides)
        .map_err(|e| match e {
            estimation25_lib::EstimationError::NoReadings => format!(
                "Erreur : MyHordes Optimizer n'a aucun relevé pour la ville {} (attaque du J{attack_day}).",
                town.id
            ),
            e => format!("Erreur : {e}"),
        })
}

fn text_input(args: &Args) -> Result<EstimationInput, String> {
    let text = read_readings(args.file.as_deref())?;
    let parsed = parse_text(&text);
    for line in &parsed.ignored {
        eprintln!("ligne ignorée : {line}");
    }
    parsed
        .into_input(&args.overrides)
        .map_err(|e| format!("Erreur : {e}"))
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    // An explicit town is enough to read from MyHordes Optimizer.
    let input = if args.api || args.town_id.is_some() {
        api_input(&args)?
    } else {
        text_input(&args)?
    };
    if input.future && !input.planner.is_empty() {
        eprintln!("relevés du planificateur ignorés : ils ne s'ajoutent qu'aux relevés du jour");
    }

    let start = Instant::now();
    let (estimate, footer) = run_search(&input, args.seeds.clone())?;
    println!("{}", format_summary(&input, &estimate));
    println!("-# ⏱️ {footer} en {:.1} s", start.elapsed().as_secs_f64());
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}
