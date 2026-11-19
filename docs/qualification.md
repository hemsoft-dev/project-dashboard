# Refresh qualification

A long-running dashboard repeats the same work all day: F5, automatic and recovery refreshes, config reloads, view
switches. A regression in task cleanup or accumulated work shows up only after many cycles. The refresh
qualification (#102, `crates/dashboard-app/src/qualification.rs`) runs one fixed workload many times and fails when
latency or retained resources leave their budgets, or when work outlives the window.

## The workload

The real `BoardView` in an in-process GPUI test window, the same harness as the headless UI tests. It never uses a
desktop window, the user's running app or the real pmd.

- **Data:**
  - 8 checkouts, each a folder with its own `origin` (`hemsoft-dev/repo-0` to `repo-7`).
  - A loopback pmd that serves each project's status (4716 B), a Board and work list of 20 items (4189 B), and the
    Fleet routes.
  - A live `Pmd` source, priorities and write backend, as in the app. Config and cache live in a temporary folder.
- **One cycle:**
  1. A user refresh of every card.
  2. A refresh while pmd answers status reads with 503: every card shows the failure and keeps its last good data.
  3. A refresh after pmd recovers: every card clears.
  4. A config replacement, applied by the config poll: one checkout leaves on even cycles and comes back on odd ones,
     and parallelism alternates between 3 and 4.
  5. Backlog, then Fleet until its data arrives, then Project.
- **Run:** 5 warm-up cycles, then 30 steady cycles. The workload is fixed: every run serves the same 5151 requests.
- **Teardown:** the window closes and the view is dropped. Then the run checks three things:
  - the view is released;
  - threads come back to the count before the window opened;
  - over 2 more simulated minutes, no request reaches pmd.

## What is measured

- **Latency:** real milliseconds from each refresh, config change or Fleet click until every card has its result,
  per kind. The harness advances GPUI's simulated clock in 100 ms steps with a 1 ms real sleep between them, so the
  numbers include that granularity. Compare them between runs of this workload, not with the app's own timings.
- **Resources:** sampled after every cycle, once everything has settled:
  - private memory: anonymous resident memory (`RssAnon`) on Linux, `PrivateUsage` on Windows;
  - OS threads of the process;
  - handles: open file descriptors on Linux, kernel handles on Windows.

  Other platforms have no probe, and a run there fails as "not measured".
- **Growth:** private memory compares the median of the last five steady samples with the median of the first
  five. Threads and handles compare every steady sample with the count after warm-up. Warm-up is excluded, so first
  allocations and caches filling up don't count as growth.
- **Process isolation:** the qualification runs alone (`--test-threads=1`, ignored by default), so the process's
  resources are its own. The fixture's request log is emptied every cycle, so the fixture doesn't count against the
  dashboard either.

## Run it

```sh
PERF_DIR=$PWD/target/perf PERF_REVISION=$(git rev-parse HEAD) \
  cargo test -p dashboard-app --locked qualification_repeated -- --ignored --test-threads=1
PERF_FIXTURE=leak ...   # the same with the controlled failure: it must exit nonzero
cargo test -p dashboard-app --locked qualification   # the budget logic, on synthetic reports
```

Each run writes `refresh-<os>-<arch>-<fixture>.json` and `.md` to `PERF_DIR`. The JSON holds the revision, platform,
build profile, workload sizes, the budgets, every cycle's raw samples, the summary and the failures. Without a full
40-character `PERF_REVISION`, or without the whole set of cycles, the run fails rather than pass on partial evidence.

`PERF_FIXTURE=leak` wraps the source so that every fetch starts a thread that never ends and leaks a megabyte.
It must fail the same budgets. Locally it reports 690 extra threads, 638 MB of growth and 805 threads left after
teardown.

## Where it runs

In the required `rust (ubuntu-latest)` and `rust (windows-latest)` jobs, on every pull request and every push to
`main`, after the tests:

1. The baseline must pass.
2. The leaking fixture must fail, with a fresh report that names this revision and lists the violations.

The reports are uploaded as `refresh-qualification-<os>-<head sha>`. Together the two runs take about a minute.

## Budgets and baselines

Measured locally on Windows 11 (debug build, three runs at `be40d79`):

| Kind | p95 (ms) |
| --- | ---: |
| refresh | 139–155 |
| failure | 67–74 |
| recovery | 141–147 |
| config | 142–154 |
| Fleet | 65–75 |

In the same three runs, memory grew 2.97–3.26 MB, threads +0, handles +0 to +1, and threads after teardown +1.

**Where the memory growth comes from.** In separate experiments, the board fed copies of the same real pmd
documents stayed flat over 250 refreshes (17.8–18.0 MB). A thread fetching from pmd in a loop grew only by the
fixture's request log. Fetches on new threads each round levelled off. The few megabytes over 30 cycles are
transport and allocator churn in the live path, not state the board keeps. The budget allows for them and catches
any per-fetch leak by orders of magnitude.

On CI (debug build, at `81eed8f`; Linux also at `ebec16b`):

| Platform | Refresh p95 per kind (ms) | Memory growth | Threads | Handles | Threads after teardown |
| --- | --- | ---: | ---: | ---: | ---: |
| ubuntu-latest | 42–121 | 1.06–1.19 MB | +0 | +0 | 0 |
| windows-latest | 51–103 | 1.15 MB | +0 | +0 | +1 |

Both served 5151 requests. The leaking fixture failed on both with 690 threads above warm-up, 806 or 807 threads
after teardown, and 623–635 MB of growth.

The budgets in `BUDGETS` (`qualification.rs`) leave room for a slow or noisy runner, and still miss a leak like the
fixture's by orders of magnitude:

| Budget | Value | From |
| --- | ---: | --- |
| Refresh p95, each kind | 1000 ms | about 8× the worst CI p95 (121 ms) |
| Private memory growth | 16 MB | about 13× CI, and 5× the local worst (3.3 MB) |
| Threads above warm-up | 4 | one set of refresh workers |
| Handles above warm-up | 32 | |
| Threads after teardown | 2 | |

A pull request that changes a budget records the new measurements it used, from both CI platforms.

## When it fails

The failure belongs to the change it measures:

- **A refresh over its p95 budget:** profile the kind that is slow. The raw samples show whether one cycle or all of
  them are slow.
- **Memory, thread or handle growth:** something survives a cycle. Compare the per-cycle series in the JSON report
  to see where it starts. Threads after teardown, a view still alive, or requests after the window closed mean
  background work isn't tied to the view's lifetime.
- **An incomplete run** (a missing revision, missing cycles or no probe on the platform): fix the run. Never pass
  on partial evidence.

Raising a budget needs the new measurements, and the reason, in the pull request. Re-measure on both CI platforms
before tightening one.
