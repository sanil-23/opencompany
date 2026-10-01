# `spawn_task` → run: dispatch, concurrency and the channel seam

An audit of what happens between a `spawn_task` tool call and work actually
running, on `upstream/main` at `45ca38a2a` (2026-10-01).

**Read the status marker.** Every claim is `verified` (read on this commit),
`observed` (seen live, not isolated) or `unverified` (reasoned, not checked) —
the same convention as
[`2026-09-24-hive-mind-unification-findings.md`](2026-09-24-hive-mind-unification-findings.md),
which this file extends. That file's "Unverified — start here" list asks *"can
the driver bind a conversation with no chat surface (a card)?"*; §5 here is the
answer.

## The one-line finding

**A spawned task never runs.** `spawn_task` writes a `todo` card and stops;
dispatch fires only on the `todo → in_progress` edge, and that edge is
reachable only from REST. No agent, tool, drain or scheduler can start a card.
And when cards *are* started, every one of them serialises on the
**company-wide** mutex — not per agent — so ten dispatches are ten blocked
tokio tasks behind one lock that also blocks every chat turn.

---

## 1. Does `spawn_task` start running? (verified: no)

Both brains write a `todo` card and return.

| Path | Site | What it writes |
| --- | --- | --- |
| Harness | `orchestrator.rs:2763` → `delegation.rs:3099` | queues `Delegation::SpawnTask`, drained at end of turn into a `TaskRecord` with `column: COLUMN_TODO` |
| Hosted | `cycle.rs:3571` | `TaskRecord` with `column: COLUMN_TODO` straight through `tasks().upsert` |

Dispatch is **edge-fired on the transition into `in_progress`**, in
`CompanyRuntime::upsert_task` (`company/runtime.rs:1182`, predicate
`task_enters_in_progress` at `:66`). A card re-saved in the column it already
occupies is an edit, not an entry — so the edge cannot be re-fired by rewriting
a card.

### Why nothing in-process can start a card (verified)

`upsert_task` is the *only* function that fires the edge, and it is reachable
from exactly three places, all of them the REST boundary:

- `server/ops/tasks.rs:575` — `POST …/tasks` (create)
- `server/ops/tasks.rs:642` — `PATCH …/tasks/{id}` (the console's column drag)
- `server/operator.rs:3029`

Every in-process path deliberately writes through the plain `TaskStore::upsert`
port instead, which has no runtime handle and therefore *cannot* dispatch: the
delegation drain (`delegation.rs:3358`), `runtime/advance.rs:32`, and the
planning settle (`planning.rs:350`) each say so in their own comments.

`AssignTaskTool` states the constraint outright
(`orchestrator.rs:3363`):

> **It does not (re)dispatch.** Dispatch fires from `CompanyRuntime::upsert_task`,
> which is reached by the console's `column → in_progress` PATCH; the delegation
> queue drains through the `TaskStore` port instead, which deliberately has no
> runtime handle. […] reaching the runtime from a tool would be a layering change.

**So the only way to start a card is an external HTTP call:**

```
PATCH /companies/{company}/tasks/{task_id}   {"column": "in_progress"}
```

### Two cosmetic defects worth fixing while here (verified)

1. The hosted tool returns `{"status": "queued", …}` (`cycle.rs:3611`). Nothing
   is queued to run. The harness tool is honest by comparison — *"It will be
   opened on the board this turn."*
2. With no harness attached, dispatch logs a **latched** warning
   (`runtime.rs:1300`) — once per runtime, not once per card. Correct for an
   inert board, but it means a misconfigured deployment reports one line for
   fifty stalled cards.

---

## 2. If there are multiple tasks, how do we run them all? (verified: one at a time, by hand)

There is no bulk runner, no sweeper and no auto-advance off `todo`.

- No batch dispatch endpoint; `PATCH` takes one `task_id`.
- The three schedulers that exist — `LifecycleScheduler` (a week-1 retention
  nudge), `WorkflowScheduler`, `CompanyScheduler` — none walks the `todo`
  column.
- The only automatic column writes *return* cards to `todo`
  (`advance.rs:191`, `:344`), never out of it.

A `deliverable: Workflow` card is the one card that routes somewhere else:
`dispatch_task` sends it to the builder pass rather than to its assignee
(`runtime.rs:1272`).

**Practical consequence:** ten spawned tasks require ten console drags. An
operator is the scheduler.

---

## 3. What does a running task actually use? (verified)

```
PATCH column=in_progress
  → CompanyRuntime::upsert_task              (fires the edge)
  → dispatch_task                            (mints the RunRecord, then tokio::spawn)
  → run_dispatch_cycle                       (loops over hand-off hops)
  → run_cycle([CompanyEvent::TaskDispatched])
  → <takes the company-wide `serial` lock>    ← see §4
  → brain.run_task                           (brain.rs:1002)
```

Inside `run_task`:

- **Who runs it** — `assignee::resolve` against the full roster (teammates,
  overlay teammates, desks). Blank assignee → the orchestrator. A name that
  resolves to nobody → `refuse_dispatch`: no turn runs, the card returns to
  `todo` with a bounce chip and the reason, and the origin thread is told.
- **A desk assignee stays a desk.** Dispatch picks which member runs *this*
  turn; it does not rewrite the card's owner (`brain.rs:1073`).
- **Which engine** — the **pooled** `CompanyAgent` from `HarnessPool`, an
  `openhuman_embed::Agent` handle. **Not the hive.**
- **Which session — isolated, already.** `isolated_session(None, _) == true`
  (`mod.rs:1962`), so a dispatch runs on a fresh
  `{company}:{agent_id}:run:{uuid}` session (`mod.rs:1548`) rather than the
  agent's stable chat session. A task run therefore neither inherits the chat
  transcript nor leaves working notes in it. **This half of the decoupling is
  done.**

The hive is structurally unreachable from a dispatch: `surface_of` needs a
`chat` to classify a surface (`hive/dispatch.rs:43`), and a dispatched turn
carries `chat_id: None`.

---

## 4. Ten tasks on one agent — queued, or waiting on the agent? (verified: neither)

**They all serialise on the company-wide mutex.**

`single_agent` decides which lock a cycle takes, and it returns `Some(agent)`
for exactly one event kind (`cycle.rs:341`):

```rust
let chat = match event {
    CompanyEvent::OperatorMessage { chat, .. } => chat.as_deref(),
    // Any other event kind may touch the whole company and so takes the
    // wide lock.
    _ => return None,
};
```

`TaskDispatched` falls into `_`. `None` then selects the wide lock
(`cycle.rs:637`):

```rust
None => self.rt.serial.clone().lock_owned().await,
```

So ten dispatches are **ten detached `tokio::spawn`s each awaiting one
`Arc<Mutex<()>>` for the whole company.** They run strictly one at a time, and
while one runs it also blocks every chat turn, every other dispatch, and every
scheduler tick in that company.

Confirmed by the existing test
`single_agent_picks_one_addressee_and_falls_back_otherwise`
(`cycle_tests_part1.rs:38`), which asserts a non-operator event yields `None`.

### The irony: the right lock exists and dispatch does not use it

Two finer-grained bounds are already built and both are bypassed:

| Lock | Grain | Used by dispatch? |
| --- | --- | --- |
| `CompanyRuntime::per_agent` (`runtime.rs:426`) | one slot per addressed agent | **No** — only addressed chat messages |
| `CompanyAgent::turn_lock` (`mod.rs:715`) | one turn per pooled agent | Never reached; `serial` blocks first |

`per_agent` exists verbatim for this problem — its own doc says *"three messages
to three agents in one company run strictly one after another even though
nothing they touch is shared"*. Task dispatch is precisely that case, and it was
left on the wide lock.

### What is absent (verified)

- **No semaphore anywhere.** `grep -rn Semaphore crates/opencompany-core/src`
  returns nothing. No concurrency cap, no queue-depth bound.
- **No timeout on the wait.** `run_bracketed`'s own doc concedes the wait is
  *"behind a busy company, an unbounded time later"*.
- **No priority.** `tokio::sync::Mutex` is FIFO, so arrival order is the only
  ordering a card gets. Card `priority` is a display field here.

A dispatch *is* checked for an emergency stop after the lock is finally
acquired (`cycle.rs:648`), so a stop engaged while cards queue does catch them —
that part is sound.

---

## 5. The channel seam: what is decoupled, and what is not

The user-facing goal is that a task carries only its **originator**, and that
completion and approvals surface back there. Status per leg:

| Leg | State |
| --- | --- |
| Run context (session) | **Done.** Isolated session per dispatch (§3). |
| Origin recorded | **Partly.** Harness stamps it; hosted does not — see below. |
| Completion → origin | **Done.** |
| Approvals → origin | **Missing.** Deliberately discarded. |

### `TaskOrigin` is already the right shape (verified)

`ports/tasks.rs:961` — one struct, one constructor, so "which conversation did
this come from" has a single answer:

```rust
pub struct TaskOrigin {
    pub origin_chat_id: String,        // the desk or chat
    pub origin_parent: Option<EventSeq>, // the thread root within it; None = channel level
}
```

A thread root without a channel is unrepresentable by construction.

### Completion already relays to the origin (verified)

`run_task` ends with `relay_reply(&card, &responder, &speaker, origin, &prior)`
(`brain.rs:1869`): the result posts into `origin_chat_id`, spoken by the
**orchestrator** (or `relay_speaker`'s pick for a DM), with the assignee
credited inside the text rather than speaking in their own voice. A card with no
origin returns `None` and simply does not relay. This is the behaviour wanted,
already working.

### Gap A — the hosted path stamps no origin (verified)

`cycle.rs:3591`:

```rust
origin: TaskOrigin::new(None, None),
```

The comment calls it intentional — *"this tool surface never recorded the
channel"*. The consequence is that **every card a hosted company spawns can
never relay its completion anywhere.** The harness path, by contrast, stamps the
raising turn's chat and thread root (`delegation.rs:3139`).

The hosted `delegate_to_desk` has the identical gap (`cycle.rs:3720`, same
`TaskOrigin::new(None, None)`), so a hosted hand-off card is equally mute. Both
sites are inside `CycleHostImpl` and both fix the same way.

Fixing this needs the hosted `spawn_task` handler to receive the cycle's
conversation. `CycleHostImpl` already holds `thread_id` / `thread_parent`
(`cycle.rs:3371`) — the values are in scope at the call site; they are simply
not read.

### Gap B — approvals discard the origin on purpose (verified)

This is the substantive missing piece. `cycle_conversation` classifies
`TaskDispatched` as a **rival trigger** and returns an empty conversation
(`cycle.rs:3180`):

```rust
CompanyEvent::TaskDispatched { .. }
| CompanyEvent::WebhookReceived { .. }
| ... => return ApprovalConversation::default(),
```

So a park raised inside a dispatched card carries `thread: None`
(`cycle.rs:3489`) and is correlated **only** by `TaskLink::Task { id }`. It
surfaces on the board card and the Approvals page
(`runtime/approval_ownership.rs`) — and **nowhere in any conversation.**

The reasoning behind that arm is sound for the case it was written for: a
dispatch batched *alongside* a chat turn must not borrow that turn's thread. But
a dispatch's own card knows its origin, and that is a different fact from the
batch's thread. The card has `origin_chat_id`; the park just never asks it.

**This is the one place where "surface approvals to the DM operator channel"
has no wiring at all.** `ParkSite.conversation` is the field to fill, from
`card.origin()` rather than from the cycle's batch.

---

## 6. Migrating off the pooled dispatch into hive logic

### What the pooled path gave the old orchestration (verified)

A dispatched card is a **background pooled turn**, and what let its work span
more than one turn was draining the delegations it queued — `run_dispatch_cycle`
loops over hand-off hops (`runtime.rs:1353`). The Sept findings record the
consequence: *"removing delegation makes a card one-shot unless something else
lets its work span turns."* A hive episode is that something else: the driver
owns the multi-turn lifecycle outright.

### The blocker, and the precedent that clears it (verified)

Episodes are conversation-keyed. `desk_id` is used as the `chat` /
`chat_id` throughout `conducted.rs` (`:204`, `:247`, `:429`), and every seat
utterance is journaled as an `AgentReply` into `chat: desk_id`. A card has no
desk.

But **`dm_hives` already mints a hive for a key that is not a manifest desk**
(`graph.rs:211`), and its shape is almost exactly what a task wants:

- key is a synthetic `dm:<owner>` string;
- **members are the entire roster** — *"the hive holds the roster, and the
  *round* holds one seat"* (`graph.rs:199`);
- lead is the owner, `ResponderMode::Lead`, so the responder is never in doubt;
- `surface_of` treats the **hive map as the authority** — an absent entry just
  means the flag is off (`dispatch.rs:58`).

That answers the Sept doc's open question in the affirmative: a hive does not
need a real chat surface, only a stable key and a bound roster. DM episodes are
**on by default** (`OPENCOMPANY_DM_EPISODES`, absent ⇒ enabled, `graph.rs:184`),
so this path is already carrying production traffic.

### The shape that follows

1. **`task_hives(record, …)` keyed `task:<task_id>`** — a direct mirror of
   `dm_hives`: members = full roster (so the card has access to every agent),
   lead = the card's resolved assignee. This is the "access to all the agents,
   not part of any chat" property, obtained by copying a function that already
   works.
2. **Reach it without `surface_of`.** `surface_of` exists to classify an
   incoming *message*. A dispatch has no message, so `run_task` should call the
   dispatcher directly with the synthetic key rather than `surface_of` growing a
   task arm it would have to fake a `chat` for.
3. **Fix the lock first, separately.** Adding `TaskDispatched` to
   `single_agent` — keyed on the *resolved assignee*, not the raw
   `card.assignee`, so a desk card keys on the member that will run — is a small
   change that is independent of the hive migration and worth strictly more than
   it. Note it needs care: a card that hands off mid-run reassigns itself, so
   the key must be taken once up front.
4. **Thread the origin into `ParkSite`** (Gap B). Also independent, also small.
5. **Decide what the `task:<id>` pseudo-channel means to the console.** An
   episode journals real `AgentReply` rows into its key. A task episode will
   therefore produce a conversation's worth of rows under a channel no console
   view renders — they must be either rendered (a per-card transcript, which is
   arguably an improvement on today's note-dump) or explicitly filtered out of
   channel lists. This is the largest unscoped piece.
6. **Answer the drain question.** `EPISODE_WITHHELD_TOOLS` strips
   `spawn_task` / `delegate_to_desk` / `delegate_to_teammate` from seats because
   *no brain drains inside an episode*. A card whose work is an episode needs
   either a drain inside the episode or an explicit decision that a task episode
   cannot open further cards.

### Unverified — do not build on these

- **Whether `tinyhivemind`'s driver accepts a key with no host chat** in
  practice. The crate is host-neutral and `dm_hives` proves a *synthetic* key
  works, but `dm:<id>` is still a key the console renders. A key nothing renders
  has not been run.
- **Whether `EpisodeBelts` seating keyed by conversation** (`hive/seating.rs`)
  behaves with a key that never appears in a chat list.
- **Whether a seat's null `memory`** is right for a card. Open in the Sept doc
  too.
- **Cost.** An episode runs rounds; a pooled dispatch runs one turn plus
  hand-off hops. Nobody has measured a card done both ways. Note the live-run
  caveat from [`dm-episode-live-findings`]: episode cost has been seen
  over-reported ~12×, which drives the spend brake — so a naive A/B will
  mislead until that is fixed.

---

## Recommended order

Smallest-first, and the first two are worth doing whatever is decided about the
hive:

| # | Change | Size | Independent of hive? |
| --- | --- | --- | --- |
| 1 | `TaskDispatched` takes the per-agent lock, not `serial` | small | yes |
| 2 | `ParkSite.conversation` ← `card.origin()` | small | yes |
| 3 | Hosted `spawn_task` stamps `TaskOrigin` | small | yes |
| 4 | A board column-move tool (`start_task`) so a card can be started without a human | small | yes |
| 5 | `task_hives` + direct dispatcher call | medium | no |
| 6 | Console story for the `task:<id>` channel | medium/large | no |

(4) is the Sept doc's own recommendation and remains unbuilt. Without it,
"decoupled task runs" still means "a human drags every card".
