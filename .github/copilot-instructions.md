# Copilot Instructions

## Build, test, and quality commands

- Local build: `cargo build`
- Local test suite: `cargo test`
- Run one test: `cargo test simulation::tests::test_attack_distribution -- --exact`
- Formatting check: `cargo fmt --check`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Lambda build used by CI/deploy: `cargo lambda build --release --arm64`

## What this project does

This is a Discord bot (slash command `/debordo`) deployed as an AWS Lambda that simulates MyHordes zombie attacks. The goal is to output, for a range of town defense values, the **probability that at least one citizen dies** during a night attack.

In MyHordes, each night zombies attack the town. If the total zombie count exceeds the town's global defense, the overflow is distributed randomly among citizens' houses. A citizen dies if they receive more zombies than their individual house defense.

### Simulation pipeline (`src/simulation.rs`)

1. **`attack_distribution(tdg_min, tdg_max, day)`** — The watchtower gives an estimated zombie count range `[tdg_min, tdg_max]`. Using the day, the game's natural attack midpoint is computed from the formula `ratio * (day * 0.75 + x)^3`. Attack values at or below the midpoint are more probable than those above (models the game's reroll mechanic). Returns a `HashMap<attack_value, probability>`.

2. **`overflow_probability(defense, ...)`** — For each attack value in the distribution, computes `overflow = attack - defense`. If positive, the overflow zombies breach the town wall and are passed to the Monte Carlo step. Result is the weighted sum of per-attack death probabilities.

3. **`debordo_sequential(day, attacking, threshold, ...)`** — Monte Carlo loop over `iterations`. Each iteration distributes `attacking` zombies among citizens via `simulate_attack`. If any citizen receives more than `threshold` (`min_def`) zombies → hit. Returns hit rate.

4. **`simulate_attack(day, attacking, drapo)`** — Distributes `attacking` zombies across `10 + 2 * ((day-10).max(0) / 2)` citizens using random normalized weights, with one randomly chosen "unlucky" citizen receiving a +0.3 weight boost.

5. **`calculate_defense_probabilities`** — Iterates over `points` defense values across `[defense_min, defense_max]` in parallel (Rayon) and calls `overflow_probability` for each.

### Key parameters

| Parameter | Meaning |
|-----------|---------|
| `defense_min/max` | Range of town defense values to evaluate |
| `tdg_min/max` | Watchtower estimate of zombie count range |
| `min_def` | Lowest house defense in town — used as uniform death threshold (conservative simplification; real citizens have varying defenses) |
| `day` | Game day — affects number of target citizens and attack probability distribution |
| `is_reactor_built` | Whether the **attacking town's** reactor is built — boosts horde by 100–250 when true |
| `nb_drapo` | Legacy flag parameter — flags are no longer used by the game but kept in code |
| `iterations` | Monte Carlo iterations per defense point (default: 10000) |
| `points` | Number of defense values sampled across the range (default: 10) |

### Code structure

- `src/main.rs` — Lambda entrypoint, Discord signature verification, request routing
- `src/simulation.rs` — All game logic and Monte Carlo simulation
- `src/config.rs` — Discord option parsing into `SimConfig`, result formatting
- `src/discord/` — Discord protocol types, Ed25519 signature verification, follow-up API helpers (unused)
- `test-events\*.json` — AWS Lambda Console fixtures (include the full API Gateway wrapper shape, not raw Discord payloads)
- `estimation25_lib/` — watchtower seed search behind `/estimation25` and the local `estimation25` CLI (see below)

### `/estimation25` (watchtower seed search)

The watchtower offsets are shrunk by PHP's `mt_rand` after `mt_srand(seed)`, with a 32-bit seed. `estimation25_lib` replays all 2^32 seeds exactly and keeps those reproducing every reading; the seed pins the hidden range and hence the attack range. Readings come from MyHordes Optimizer (`town_id` + `day`, or the town of the key registered with `/register-key`) or are pasted in a modal (`coller`, or when neither is available).

- **Fan-out**: the receiver enqueues a *plan* job; the worker resolves the readings, validates them (`check_input`), creates a run item in DynamoDB and starts `ESTIMATION_PARTS` (default 128) *part* jobs, one per `seed_slices` slice, as asynchronous invocations of the worker itself (`AWS_LAMBDA_FUNCTION_NAME`, `SqsEvent`-shaped payload): the SQS trigger adds pollers only a few per second, so a large fan-out through the queue started its last parts ~14 s late. The watchdog still goes through SQS. Each part runs `search_seeds` on its slice, reduces its windows to a `Summary` (`summarize`: attack range and run count; a window's attack doesn't depend on the other windows, so summaries merge exactly) and records it in the run item with an idempotent `UpdateItem` (a loose search finds thousands of windows per part, beyond the item's 400 KB); the part that completes the run (and wins the conditional `posted` flag) merges the summaries and posts the result. While searching, every part reports its searched-seed count to the run item every 5 s (`report_estimation_progress`); a conditional `last_edit_at` claim lets at most one part per 5 s edit the waiting message (rotating phrase + progress bar). Reports stop once the part is recorded or the run is posted, and a part finishes its edit before recording itself, so a progress edit can never overwrite the result.
- **Manual infra** (like the SQS queue): DynamoDB tables `EstimationRuns` / `EstimationRuns-dev` (partition key `run_id`, string; TTL on `expires_at`); the Lambda role needs `dynamodb:PutItem`/`UpdateItem` (worker) and `GetItem` + `UpdateItem` (receiver: the result's "Voir la configuration" button reopens the paste modal with the run's stored `config` (town, attack day and every red-soul setting, then the readings; submitted without readings but with `town_id` + `jour`, the readings are fetched again from MyHordes Optimizer); the waiting message's "Annuler" button, caller only, sets the run's `posted` flag so its parts cancel their search at their next progress check) on them, and `sqs:SendMessage` plus `lambda:InvokeFunction` on the worker functions for the worker; the SQS trigger of the worker should use **batch size 1** so jobs run in parallel. The parts bypass that trigger, so only the account's concurrency (1000) bounds them: give the receiver reserved concurrency so a search never throttles commands.
- **Tuning**: `power-tuning/` holds AWS Lambda Power Tuning payloads (`EstimationStage::Bench`: the worker searches one slice without DynamoDB nor Discord, and a failed bench fails the invocation) and the runbook to choose the worker's memory and `ESTIMATION_PARTS` (chosen: 2560 MB, set by `--memory` in the deploy workflows, and 128 parts; results in the runbook). The worker logs how long each part waited to start (`waited N ms to start`) and, when a run completes, its merged summary. `cargo lambda build --release --arm64` already applies `target-cpu=neoverse-n1`, `codegen-units=1`, thin LTO and `panic="abort"` to the Lambda build only (the CLI's `--release` build is plain).
- Local CLI: `cargo run --release -p estimation25_lib --bin estimation25 -- --town_id ID --jour N` (or a readings file). The Lambdas depend on `estimation25_lib` with `default-features = false` (no blocking HTTP client).

## Intentional simulation behaviors (do not "fix")

- **Fractional defense values**: defense is always an integer in MyHordes, but the step formula in `calculate_defense_probabilities` can produce fractional defense values when `points` doesn't divide the range evenly. The truncation in `overflow_int = overflow as i32` is acceptable because users should pass `points = (defense_max - defense_min) + 1` to get exact integer steps. Fractional intermediate values are just curve-sampling approximations.
- **Rounding over-allocation in `simulate_attack`**: after distributing zombies using normalized random weights, the per-cell integer rounding can make `sum(allocated) > leftover`. This is intentional — in MyHordes the game engine rounds each cell independently, which can produce more zombies than the theoretical overflow value. The `while attacking_cache > 0` loop only corrects under-allocation; silent over-allocation is by design.

## Key repository conventions

- Keep slash-command parameter names and defaults aligned across `src/config.rs`, `register_command.py`, and the Lambda Console fixtures in `test-events\`. Changes to the command schema usually require edits in all three places.
- Tests live inside modules compiled through **both** binary crates (`bootstrap` and `worker`), so targeted test runs should use module-path filters like `simulation::tests::...`.
- `SKIP_SIGNATURE_CHECK=true` is the test-mode escape hatch for Lambda Console fixtures. Normal request handling expects `DISCORD_PUBLIC_KEY` and `SQS_QUEUE_URL` to be set.
- The receiver Lambda (`bootstrap`) only verifies the signature, returns a deferred response (type 5), and enqueues a `SimulationJob` to SQS. It never runs the simulation.
- The worker Lambda (`worker`) is SQS-triggered, runs the simulation in `spawn_blocking`, and PATCHes the Discord interaction via `discord::api::send_followup`. It must have a 300s timeout configured in AWS (set in `deploy.yml`). For `/estimation25` it also needs `SQS_QUEUE_URL` (to enqueue the parts) and `ESTIMATION_TABLE_NAME`, both set in the deploy workflows.
- `SQS_QUEUE_URL` must be added as a GitHub Actions secret. The SQS queue visibility timeout must also be ≥ 300s (configured manually in AWS Console when creating the queue).
- User-facing command descriptions and output text are written in French; keep additions and edits consistent with the existing language.
- Deployment builds both binaries via `cargo lambda build --release --arm64` and deploys them separately: `DebordoLambda` (receiver) and `DebordoLambdaWorker` (worker).
