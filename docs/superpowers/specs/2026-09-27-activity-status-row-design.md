# Activity status row

Issues: alacritree/alacritree#317 (indicate background job progress), groundwork for alacritree/alacritree#316 (jobs and diagnostics view).

## Problem

Pressing a bound refresh shows nothing. `RefreshPrStatus` (`app/git_panel.rs`) marks every PR cache entry stale and requests a repaint; the lookups start on a later frame, run as one pooled job that walks repositories in sequence (`alacritree_gh::pull_requests`), and land all at once. `RefreshProjects` and the per-project refresh button start `project_refreshes` jobs that draw no pending state either. Until results arrive the refresh is indistinguishable from a keypress that did nothing, and a refresh that fails looks the same as one that found nothing new.

## Goal

Work the user triggers shows that it is running, how far along it is, roughly how long it has left, and how it ended.

Success means:

- Pressing the refresh key changes something on screen on the next frame.
- A PR refresh across several repositories counts them off as each finishes, with no input from the user.
- From the second refresh after launch, the row shows an estimate of the time left.
- A refresh that fails says so and says why, and the message stays until the next result replaces it.

## Scope

In scope: work started by a keypress, click or palette command. That is `RefreshPrStatus`, `RefreshProjects`, and the per-project refresh button.

Out of scope:

- Timer-driven work: git status polls, herdr and zellij polls, liveness probes, the PR cache's own TTL re-queries. They run every few seconds and would make the row flicker.
- The #316 jobs panel and `doctor` output. The progress readers below are what such a panel would read, but no panel is built here.
- Persisting timings across launches.
- A config option. The row is new UI that changes nothing the user already does.

## Design

### Progress on the job pool

`alacritree_common::jobs` gains progress reporting on the worker side and a reader on the app side.

A job declares its steps up front and then reports each one as it ends:

- `Blocking::set_steps(labels: Vec<String>)` names every step the job will run, in order. Labels are unique within a job. Calling it again replaces the list.
- `Blocking::step_done(label: &str, outcome: Result<(), String>)` records that step's outcome and its duration. The duration runs from the previous `step_done`, or from `set_steps` for the first step, so work done before `set_steps` is not charged to any step.
- `step_done` runs the pool's registered wake-up, the same one `WakeOnEnd` runs, so the frame that reads the new count happens without user input. `Blocking` carries that wake-up from its pool. `on_this_thread` has none, and its `step_done` only records.

Both write to an `Arc<Mutex<Progress>>` shared by the job's `Blocking`, its `Job<T>`, and any readers:

- `Job::progress_reader() -> ProgressReader` hands out a cheap clone. A reader outlives the job, so the code that owns the `Job` (here `PrCache`'s private `Batch`) keeps ownership and cancel-on-drop, and the activity reads through its own reader.
- `ProgressReader::snapshot() -> ProgressSnapshot` returns `steps: Vec<Step>` in declared order and `end: Option<JobEnd>`. A `Step` is `label`, `outcome: Option<Result<(), String>>` and `took: Option<Duration>`. `JobEnd` is `Returned`, `Panicked` or `Cancelled`.
- The worker sets `end` when the closure returns or unwinds. Dropping the `Job` sets `Cancelled` if nothing else has.
- A job that never calls `set_steps` reports no steps. `Job::ready`, `Job::never` and `Job::panicked` report the same, with the matching `end`.

`Blocking` stays constructible only inside the pool, so the progress calls stay worker-only like every other `Blocking` method. A test-support helper, `jobs::recorded(f: impl FnOnce(&Blocking) -> T) -> (T, ProgressSnapshot)`, runs `f` on the calling thread the way `on_this_thread` does and returns what it reported.

### Progress from the GitHub backend

`alacritree_gh` gets a seam around the loop in `pull_requests`:

```rust
fn pull_requests_with(
    heads: Vec<Head>,
    blocking: &Blocking,
    resolve: impl Fn(&Path) -> Option<(String, String)>,
    request: impl Fn(&Path, &str) -> Option<Vec<u8>>,
    per_branch: impl Fn(&Head, Option<&str>) -> Result<Option<PrInfo>, ForgeError>,
) -> PullRequests
```

`GhForge::pull_requests` calls it with `resolve_repo`, `graphql::run` and `query_gh`, which is what it does inline today. Inside it:

1. `groups_with(heads, resolve)` builds the groups. Resolution costs one `gh repo view` per repository and runs before any steps exist.
2. `blocking.set_steps` names one step per group.
3. After each group's `query_group`, `blocking.step_done` reports it. The step fails when any member came back `Err`, carrying the first member's `ForgeError` text. The cancel check between groups stays where it is.

Step labels:

- A resolved group is `owner/repo`. Chunks of one repository after the first add their position, `owner/repo (2)`, so labels stay unique and each chunk keeps its own timing.
- A per-branch group is its `cwd` through `wsl::display_path`, so a WSL checkout reads the way the sidebar shows it.

The `RemoteForge` trait does not change.

`FakeForge` reports the same way: one step per head, labelled by branch, failing with its `ForgeError` text on a `failing_on` branch.

### Activities

A new `activity.rs` module in the app crate owns what the row shows. `AlacritreeApp` holds one `Activities`.

- `ActivityKind` is `PrStatus` or `ProjectScan`.
- A `PrStatus` activity holds a `ProgressReader` for each batch started on its behalf. Its steps are the union of their steps.
- A `ProjectScan` activity holds a set of project roots, each pending or finished with a `Result<(), String>`.
- `Activities::tick(now)` runs once per frame. It reads each reader, folds newly finished PR steps into the timing table, and ends activities whose end condition holds (see below).
- The timing table is `HashMap<String, Duration>`, the most recent duration of each PR step label. A failed step does not overwrite its entry.
- The estimate for a running PR activity is the sum of table durations for its steps with no outcome. Steps with no recorded duration add nothing. The estimate is shown only when at least one pending step has a recorded duration. Groups inside one batch run in sequence, so the sum fits the common case of one batch per refresh and overstates the rare case of two batches running at once.
- Project scans run in parallel on the pool, so a sum of durations would mislead. They show a count and no estimate.
- A trigger of a kind that is already running joins the running activity instead of starting a second.
- The last result is one value: finished (kind, when) or failed (kind, when, failed count, total count, first error, every failed label with its error). A newer result of either kind replaces it.
- `Activities` takes an injected clock, as `PrCache::with_clock` does, so tests control time.

### Wiring the PR refresh

`RefreshPrStatus` does not start a job, so the activity cannot hold one at trigger time.

- With `[integrations.gh] pr_status` off, the action sets the last result to "PR status is off" and does nothing else. No flag is raised and no activity starts.
- Otherwise the action calls `invalidate_all` as today, raises a `triggered` flag on `PrCache`, and starts or joins a `PrStatus` activity.
- While the flag is up, `spawn_due` hands each batch's `ProgressReader` to the activity.
- Lookups are queued by the sidebars' `poll` calls during paint and started by the next frame's `drain_completed`, which runs before input handling and paint (`poll_update_jobs`). A key binding triggers before that frame's paint, a palette command after it. So the flag drops only at a drain that follows at least one sidebar paint since the trigger, and finds no batch in flight and nothing due.
- The activity ends when the flag drops, and its result comes from its readers:
  - No steps at all: "nothing to check". With both sidebars hidden nothing polls, and that is the answer the row gives.
  - A reader whose `end` is `Panicked` marks its steps with no outcome failed with "worker panicked". A panic before `set_steps` counts as one failed step labelled "PR lookup".
  - A reader whose `end` is `Cancelled` marks its steps with no outcome failed with "timed out". `PrCache` drops a batch only once it has been in flight past the TTL, so that is what a cancel means here.
  - Any failed step makes the result a failure. Otherwise it is finished.

### Wiring project scans

`RefreshProjects` and the per-project refresh button go through `refresh_project`. A trigger starts or joins a `ProjectScan` activity and registers each root it refreshes, whether `project_refreshes.start` spawned a new job or the root already had one running. A scan already under way when the user asks is the one that answers them.

`poll_project_refreshes` reports each root it takes from `take_finished` to the activity. The worker panicking is a failure with "the project refresh worker panicked". A project removed before its scan finishes leaves the total. The activity ends when every registered root has finished or left.

Scans started by other paths, such as `refresh_moved_branches` or startup, are not registered.

### The row

The row is the last line of the left sidebar. Inside `SidePanel::left("left_sidebar")` it is drawn as a bottom panel before `tasks_panel::show`, because nested bottom panels stack upward from the first one drawn. So it sits below the tasks panel when that panel docks left. It keeps its height whether or not it has content, so the project list above it does not move.

A pure `status_line(&Activities, now) -> StatusLine` produces what the row paints. The painter only lays it out.

| State | Text |
|---|---|
| PR refresh, steps known, estimate available | `PRs 2/4 · ~3s` |
| PR refresh, steps known, no estimate | `PRs 0/4 · 1s` (elapsed) |
| PR refresh, no steps yet | `PRs · 1s` (elapsed) |
| Project scan | `Scanning projects 3/7` |
| Two activities running | the newer one's text, then `+1` |
| Last result, PR success | `PRs refreshed 2m ago` |
| Last result, PR failure | `PR refresh: 2 of 9 failed: <first error>` |
| Last result, scan success | `Projects scanned 2m ago` |
| Last result, scan failure | `Project scan: 1 of 7 failed: <first error>` |
| Nothing to check | `PRs: nothing to check` |
| PR status disabled | `PR status is off` |
| Nothing triggered since launch | blank |

Running states lead with the sidebar's `braille_loader` in `theme.accent`. Text is small and `theme.text_dim`, failures use the theme's error colour.

Hovering shows a tooltip with one line per running activity. A PR activity lists each step label with a tick once done. A failure lists every failed label with its error. The row has no other interaction.

Repaints:

- `step_done` wakes the UI, so a count changes on the frame after the step ends.
- While an activity runs, the row calls `request_repaint_after(1s)` so the elapsed time and the spinner move between steps.
- While a success age is shown, it calls `request_repaint_after(60s)`.
- Otherwise it asks for nothing. The row never drives repaints at frame rate.

### Edge cases

- A hidden left sidebar hides the row. Nothing else shows progress. Showing the sidebar again shows the current state.
- A second `RefreshPrStatus` while one runs joins it. `invalidate_all` marks everything stale again, and a batch already in flight keeps running and counts toward the same activity.
- A step label seen again in a later refresh overwrites its table entry only when that step succeeds.

## Testing

Each test goes through the interface its caller uses, and is written first and watched failing.

1. `jobs.rs`: a pooled job that calls `set_steps` with three labels and `step_done` twice reports two finished steps with durations, one pending, and `end: None` through a reader; the pool's waker runs on each `step_done`; after the job returns, `end` is `Returned`; dropping the `Job` of a job that has not ended sets `Cancelled`.
2. `alacritree_gh`: `pull_requests_with` under `jobs::recorded`, with injected `resolve`, `request` and `per_branch`, reports one step per group with the labels above, including `owner/repo (2)` for a second chunk, and a failed step carrying the per-branch error.
3. `activity.rs`, with an injected clock and hand-built readers: the estimate sums pending step durations; a second trigger joins the running activity; a newer result replaces the last one; a failure counts failed steps and carries the first error; a failed step keeps the previous duration; a panicked reader's pending steps fail with "worker panicked" and a cancelled one's with "timed out"; a scan root that was already running counts toward the activity.
4. `pr_status.rs`: batches started while `triggered` is up hand their readers over; the flag stays up at a drain before any paint and drops at the first drain after a paint that finds nothing in flight or due.
5. End to end: `Forge` gets a `#[cfg(test)] Fake(FakeForge)` variant. Run the `RefreshPrStatus` action on an app built with it, drive frames, and assert `status_line` moves from `PRs 0/1` to `PRs refreshed`, and with a `failing_on` branch to `PR refresh: 1 of 1 failed`.
