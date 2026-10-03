# Tuning `/estimation25` with AWS Lambda Power Tuning

The 2^32-seed search of `/estimation25` is split into `ESTIMATION_PARTS` part jobs on the worker
Lambda. Each part is CPU-bound rayon work, and Lambda gives a function vCPUs in proportion to
its memory (1769 MB = 1 vCPU, 10240 MB = 6 vCPUs). Two values set how long a run takes: the
worker's memory and the number of parts.

The payloads here are `Bench` jobs (`EstimationStage::Bench`): the worker searches the first of
`parts` seed slices, logs `Estimation bench: … in X s on N thread(s)`, and touches neither
DynamoDB nor Discord. If a bench fails or times out, the invocation fails.

| File | Slice searched | Use |
|------|----------------|-----|
| `payload.json` | 1/8 of the seeds (one real part today) | fan-out constants, at the chosen memory |
| `payload-p64.json` | 1/64 of the seeds | memory sweep (short enough for `num: 5`) |

Both use the readings of `estimation25_lib/tests/data/j15_real_attack_2587.txt`. To regenerate
them after changing the job schema, run
`cargo test --bin worker generate_power_tuning_payloads -- --ignored`. The test
`test_power_tuning_payloads_are_bench_jobs_of_the_fixture` fails while they are stale.

## 0. Prerequisites (Service Quotas, eu-west-3)

- **Function memory**: new accounts can be capped at 3008 MB. Request 10240 MB if it is capped.
- **Concurrent executions**: request an increase (default 10). Dev and prod share this pool.
- Deploy **aws-lambda-power-tuning** from the Serverless Application Repository with
  `totalExecutionTimeout = 900` and `lambdaResource` set to the ARN of
  `DebordoLambdaWorker-dev`. It changes the memory of the function it tunes, so never point it at
  prod.

## 1. Memory sweep

Start an execution of the state machine with this input:

```json
{
  "lambdaARN": "arn:aws:lambda:eu-west-3:<ACCOUNT>:function:DebordoLambdaWorker-dev",
  "powerValues": [1769, 3008, 5307, 7076, 8845, 10240],
  "num": 5,
  "parallelInvocation": false,
  "strategy": "speed",
  "autoOptimize": false,
  "payload": <contents of payload-p64.json>
}
```

- Drop the values above your memory quota.
- `autoOptimize: false`: the deploy workflows own the memory setting.
- `parallelInvocation: false`: the 5 invocations run one after another and use only one
  concurrent execution. They must fit in the 900 s executor timeout, which is why this sweep uses
  the 1/64 slice.

Read the result as `duration × vCPUs`. While it stays flat, the extra memory buys proportional
speed. Choose the fastest size (**M**) whose result is still at least ~10% better than the next
smaller size.

## 2. Fan-out constants at M

Run two executions with `"powerValues": [M]`: one with `payload.json` (average duration `d8`) and
one with `payload-p64.json` (`d64`). A part over 1/P of the seeds takes `d(P) = a + b/P`, so:

```
b = (d8 - d64) * 64 / 7     # the whole 2^32 search on one part at M
a = d64 - b / 64            # fixed cost of a part
```

If 5 × `d8` exceeds 900 s, use `"num": 5, "parallelInvocation": true` for that run. That uses 5
concurrent executions.

## 3. End-to-end runs on the dev bot

For P in 8, 16, 32 and 64 (and each P must fit within the concurrency quota):

1. On `DebordoLambdaWorker-dev`, set memory M and the env var `ESTIMATION_PARTS=P`, and set the
   maximum concurrency of its SQS trigger to P. The next dev deploy resets these settings, so do
   the runs before it.
2. Run `/estimation25` with the same readings (e.g. paste the j15 fixture). Check that the result
   matches the 8-part result.
3. Note the "runs testées en N s" footer, then read the worker logs (CloudWatch Logs Insights,
   `filter @message like "waited" or @message like "complete:"`):
   - `Estimation part i/P of <run> waited N ms in the queue`: the largest wait is the start-up
     ramp. The SQS trigger starts 5 pollers and adds up to 300 per minute.
   - `Estimation run <run> complete: P part(s), N bytes of matches in its item`: the item is
     limited to 400 KB and rewritten by every 5 s progress report.

## 4. Choosing P and applying it

- Choose the smallest P whose footer time is within ~10% of the best one.
- Two conditions trigger a code change before going further:
  - **Item size**: if the matches near ~100 KB, move each part's matches to its own item.
  - **Ramp**: if the queue ramp is a large share of the run, fan the parts out with
    asynchronous `lambda:Invoke` instead of SQS.
- Apply M and P to the "Deploy Worker Lambda" steps of both workflows (`--memory M`,
  `--env-var ESTIMATION_PARTS=P`) and to `DEFAULT_ESTIMATION_PARTS` in `src/worker.rs`. Set the
  maximum concurrency of both SQS triggers to P.
- Once at least 100 executions stay unreserved, give the receiver reserved concurrency (e.g. 10)
  so that a running search never throttles commands.
- Cost: one run uses ≈ `P × M/1024 × (a + b/P)` GB-s. Compare it with the 400 000 GB-s/month
  free tier.
