//! Throughput of the seed search on a readings file.
//!
//! `cargo run --release -p estimation25_lib --example seed_bench -- FIXTURE [SEEDS]`
//! (set `RAYON_NUM_THREADS=1` for single-thread numbers).

use estimation25_lib::parse::{InputOverrides, parse_text};
use estimation25_lib::{EstimConf, estimate};
use std::sync::atomic::AtomicU64;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: seed_bench FIXTURE [SEEDS]");
    let seeds: u32 = args
        .next()
        .map_or(100_000_000, |s| s.parse().expect("SEEDS must be a u32"));

    let text = std::fs::read_to_string(&path).expect("cannot read fixture");
    let input = parse_text(&text)
        .into_input(&InputOverrides::default())
        .expect("invalid readings");

    let progress = AtomicU64::new(0);
    let start = Instant::now();
    let result = estimate(&input, &EstimConf::default(), 0..=seeds - 1, &progress);
    let secs = start.elapsed().as_secs_f64();

    let rate = f64::from(seeds) / secs;
    println!(
        "{path}: {seeds} seeds in {secs:.2} s = {:.1} M seeds/s, full sweep ≈ {:.0} s, matches: {}",
        rate / 1e6,
        (f64::from(u32::MAX) + 1.0) / rate,
        result.map_or(0, |r| r.seeds.len())
    );
}
