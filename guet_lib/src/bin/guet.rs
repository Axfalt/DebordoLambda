//! Local CLI: `guet [--jour N] [--demain] [--mode normal|hard|easy] [--particles N] [--seed S]
//! [--exact [--seeds A-B]] [--top K] [--csv FILE] [FICHIER]`. Readings are read from FICHIER, or
//! stdin when omitted. `--exact` brute-forces the 2^32 PHP seeds instead of the particle filter.

use guet_lib::parse::{InputOverrides, parse_bool, parse_mode};
use guet_lib::{InferenceOptions, Posterior, format_summary, infer, infer_exact, parse_text};
use std::io::Read;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const USAGE: &str = "Usage : guet [--jour N] [--demain] [--mode normal|hard|easy] [--particles N] \
[--seed S] [--exact [--seeds A-B]] [--ames N] [--ames-veille N] [--penalite 0.04] [--ames-max 1.2] \
[--top K] [--csv FICHIER] [RELEVÉS]\n\
Les relevés (une ligne `33% 2047 - 2749` par citoyen) sont lus depuis RELEVÉS, ou l'entrée standard.";

struct Args {
    day: Option<i64>,
    future: Option<bool>,
    mode: Option<guet_lib::AttackMode>,
    particles: Option<usize>,
    seed: Option<u64>,
    top: usize,
    csv: Option<String>,
    file: Option<String>,
    exact: bool,
    seeds: std::ops::RangeInclusive<u32>,
    red_souls: Option<u32>,
    planner_red_souls: Option<u32>,
    soul_penalty: Option<f64>,
    soul_max: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        day: None,
        future: None,
        mode: None,
        particles: None,
        seed: None,
        top: 5,
        csv: None,
        file: None,
        exact: false,
        seeds: 0..=u32::MAX,
        red_souls: None,
        planner_red_souls: None,
        soul_penalty: None,
        soul_max: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} attend une valeur"));
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.to_string()),
            "--jour" | "--day" => {
                args.day = Some(
                    value("--jour")?
                        .trim_start_matches(['J', 'j'])
                        .parse()
                        .map_err(|e| format!("--jour : {e}"))?,
                )
            }
            "--demain" | "--j1" => args.future = Some(true),
            "--mode" => {
                let v = value("--mode")?;
                args.mode = Some(parse_mode(&v).ok_or(format!("mode inconnu : {v}"))?);
            }
            "--particles" => {
                args.particles = Some(
                    value("--particles")?
                        .parse()
                        .map_err(|e| format!("--particles : {e}"))?,
                )
            }
            "--seed" => {
                args.seed = Some(
                    value("--seed")?
                        .parse()
                        .map_err(|e| format!("--seed : {e}"))?,
                )
            }
            "--top" => {
                args.top = value("--top")?
                    .parse()
                    .map_err(|e| format!("--top : {e}"))?
            }
            "--csv" => args.csv = Some(value("--csv")?),
            "--ames" => {
                args.red_souls = Some(
                    value("--ames")?
                        .parse()
                        .map_err(|e| format!("--ames : {e}"))?,
                )
            }
            "--ames-veille" => {
                args.planner_red_souls = Some(
                    value("--ames-veille")?
                        .parse()
                        .map_err(|e| format!("--ames-veille : {e}"))?,
                )
            }
            "--penalite" => {
                args.soul_penalty = Some(
                    value("--penalite")?
                        .replace(',', ".")
                        .parse()
                        .map_err(|e| format!("--penalite : {e}"))?,
                )
            }
            "--ames-max" => {
                args.soul_max = Some(
                    value("--ames-max")?
                        .parse()
                        .map_err(|e| format!("--ames-max : {e}"))?,
                )
            }
            "--exact" => args.exact = true,
            "--seeds" => {
                let v = value("--seeds")?;
                let (a, b) = v
                    .split_once('-')
                    .ok_or(format!("--seeds attend A-B : {v}"))?;
                let parse = |x: &str| {
                    x.trim()
                        .parse::<u32>()
                        .map_err(|e| format!("--seeds : {e}"))
                };
                args.seeds = parse(a)?..=parse(b)?;
            }
            s if s.starts_with("--demain=") => {
                args.future = Some(
                    parse_bool(&s["--demain=".len()..]).ok_or(format!("valeur invalide : {s}"))?,
                )
            }
            s if s.starts_with('-') => return Err(format!("option inconnue : {s}\n{USAGE}")),
            s => args.file = Some(s.to_string()),
        }
    }
    Ok(args)
}

fn histogram(post: &Posterior, bins: usize, width: usize) -> String {
    let (lo, hi) = post.support();
    let bin_size = (((hi - lo + 1) as f64 / bins as f64).ceil() as i64).max(1);
    let n_bins = ((hi - lo) / bin_size + 1) as usize;
    let mut mass = vec![0.0; n_bins];
    for &(v, p) in &post.attack {
        mass[((v - lo) / bin_size) as usize] += p;
    }
    let peak = mass.iter().copied().fold(0.0, f64::max);
    let mut out = String::new();
    for (i, m) in mass.iter().enumerate() {
        let start = lo + i as i64 * bin_size;
        let bar = "█".repeat(((m / peak) * width as f64).round() as usize);
        out.push_str(&format!(
            "{:>6}-{:<6} {:>5.1} % {bar}\n",
            start,
            start + bin_size - 1,
            m * 100.0
        ));
    }
    out
}

/// Seed brute force with a progress line on stderr; returns the posterior and a footer.
fn run_exact(
    input: &guet_lib::GuetInput,
    opts: &InferenceOptions,
    seeds: std::ops::RangeInclusive<u32>,
) -> Result<(Posterior, String), String> {
    let total = *seeds.end() as u64 - *seeds.start() as u64 + 1;
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
                    "\rgraines : {:5.1} % ({secs:.0} s, reste ~{eta:.0} s)   ",
                    p * 100.0
                );
            }
            eprintln!();
        });
        let result = infer_exact(input, &opts.conf, seeds, &progress);
        done.store(true, Ordering::Relaxed);
        result
    });
    let exact = result.map_err(|e| format!("Erreur : {e}"))?;
    for m in &exact.matches {
        eprintln!(
            "graine {:#010x} : offsets ({}, {}), tmin {}-{}, tmax {}-{}",
            m.seed, m.om0, m.ox0, m.tmin.0, m.tmin.1, m.tmax.0, m.tmax.1
        );
    }
    let footer = format!(
        "{} graine(s) compatible(s) sur {total} testées",
        exact.matches.len()
    );
    Ok((exact.posterior, footer))
}

fn run() -> Result<(), String> {
    let args = parse_args()?;
    let text = match &args.file {
        Some(path) => {
            std::fs::read_to_string(path).map_err(|e| format!("lecture de {path} : {e}"))?
        }
        None => {
            eprintln!("Collez les relevés puis Ctrl-Z/Entrée (Windows) ou Ctrl-D :");
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(|e| e.to_string())?;
            buf
        }
    };

    let parsed = parse_text(&text);
    for line in &parsed.ignored {
        eprintln!("ligne ignorée : {line}");
    }
    let input = parsed
        .into_input(&InputOverrides {
            day: args.day,
            future: args.future,
            mode: args.mode,
            red_souls: args.red_souls,
            planner_red_souls: args.planner_red_souls,
            soul_penalty: args.soul_penalty,
            soul_max: args.soul_max,
        })
        .map_err(|e| format!("Erreur : {e}"))?;
    if input.future && !input.planner.is_empty() {
        eprintln!("relevés du planificateur ignorés : ils ne s'ajoutent qu'aux relevés du jour");
    }
    let mut opts = InferenceOptions::default();
    if let Some(p) = args.particles {
        opts.particles = p;
    }
    if let Some(s) = args.seed {
        opts.seed = s;
    }

    let start = Instant::now();
    let (post, footer) = if args.exact {
        run_exact(&input, &opts, args.seeds.clone())?
    } else {
        let post = infer(&input, &opts).map_err(|e| format!("Erreur : {e}"))?;
        let footer = format!(
            "{} hypothèses cachées évaluées, {} particules chacune",
            post.hypotheses, opts.particles
        );
        (post, footer)
    };
    let elapsed = start.elapsed();

    println!("{}", format_summary(&input, &post, args.top));
    println!(
        "📊 Distribution de l'attaque :\n{}",
        histogram(&post, 24, 40)
    );
    println!("-# {footer}, {:.2} s", elapsed.as_secs_f64());

    if let Some(path) = &args.csv {
        let mut csv = String::from("attack,probability\n");
        for (v, p) in &post.attack {
            csv.push_str(&format!("{v},{p}\n"));
        }
        std::fs::write(path, csv).map_err(|e| format!("écriture de {path} : {e}"))?;
        eprintln!("distribution écrite dans {path}");
    }
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
