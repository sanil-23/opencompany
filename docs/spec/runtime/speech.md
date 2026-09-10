# One agent, one session — and talking as a tool call

Two changes that only make sense together: an agent that is continuous, and an
agent that speaks by calling a tool.

## Before

One `oh::agent::Agent` is held per `(company, agent_id)`. Its in-memory history
was **cleared and re-seeded from the incoming desk's transcript every time the
chat changed** (`bound_chat`, `src/harness/built_in/mod.rs`). That kept two
conversations from bleeding into each other, and it meant an agent had no
continuous existence: it could not notice that the question just asked in a DM
is the one it answered on a desk an hour ago, because between the two it had
been emptied.

Speaking was not a tool either. Every other thing an agent can do is one — it
writes a ledger row with `record_entry`, opens a card with `spawn_task` — but a
turn's **return text** was the message, journaled on its behalf. So it could not
choose a recipient, could not say something to one teammate rather than to the
room, and could not decline to speak; the only way to stay quiet was to return
an empty string, which reads as a failed turn.

## The session

`src/harness/built_in/agent_session.rs`. The session is never cleared. Each turn
the agent is handed the rows it has **not yet seen**, across **every** channel it
can read, each cued with where it was said:

```text
While you were away, this was said elsewhere in the company. It is context,
not a request — answer the message at the end of this turn.

[#Brand · copy] Tone should be warmer.
[dm · ceo] What did design decide?
```

`AgentSessionState` is a **watermark**, modelled on
`tinyhivemind::sharing::SharingState` minus its `conversation` field — that field
is what makes the vendored type return `ReinitializeReason::ConversationChanged`
on a channel switch, which is the exact behaviour being removed.

There is no mailbox and no per-agent queue, because `tinyhivemind` refuses to
have one: `NoDispatchReason::SelfMention`, `NoReferralReason::SelfMention` and
`UtteranceRejection::SelfRecipient` all decline self-addressing by name. The
architecture is **stigmergic** — work leaves an attributed trace in a shared,
globally sequenced log, and the trace is the stimulus for the next turn.
Continuity is a watermark the host owns, not a message an agent posts to itself.

### What is given up, and what is not

| | |
|---|---|
| **Channel isolation** | Given up, deliberately. This is what `#1725` / `#1730` / `#1890` installed, and the replacement is the cue: every delivered row is prefixed with its channel, per line, through the same `prefix_every_line` machinery that already defends attribution against forgery. |
| **Audience isolation** | **Kept.** A private aside this agent is not party to still never reaches it. `readable_by` applies the same narrowing `EpisodeDriver` applies with `Viewer::Agent`. |
| **A bound on growth** | Kept, and stated: `SESSION_DELTA_LIMIT` (60 rows) and `SESSION_SCAN_LIMIT` (2048 raw rows). Crossing either is a `Reinitialize`, which falls back to the recent-window seed — the honest answer when a partial replay would be worse context than a fresh window. |

The ordering is a contract: **commit `next_state` only after the agent has
accepted the rows.** Committing first would let a failed turn leave the session
believing it read something it never saw.

## Talking as a tool call

`src/harness/speech_tools.rs`. Off unless the manifest says so:

```toml
[speech]
enabled = true
```

**Company-level, not per-desk.** `[group_chat.hive.aside]` and
`[group_chat.hive.referral]` nest under a desk because they are properties of a
deliberation on that desk. Speech is a property of an agent's *session*, which
spans every desk it sits on plus its DM plus the General line. A per-desk knob
would let one agent speak by tool call on one desk and by return text on another
inside one unbroken session.

The names, argument shapes and description text all come from
`tinyhivemind::speech`, which states them once as data and asks a host to render
them **verbatim**. Nothing here invents a contract.

| Tool | Effect |
|---|---|
| `desk_post { message }` | Say one thing to the whole channel. Collected, not appended — see below. |
| `desk_dm { to, message }` | Say one thing to named teammates. Journals itself with `AgentReply.audience` set. |
| `desk_close { message }` | Say one last thing and report the work finished. |
| `desk_read { limit }` | Read further back in this channel than the turn was handed. Clamped by the crate's own `READ_MAX`. |

The names are prefixed because the crate's are bare and it anticipates exactly
this: *"an MCP server called `desk` serving `post` presents it as `desk_post`,
and the descriptions are written to read correctly either way."* `read` and
`post` unqualified would sit beside `read_thread`, `read_ledger` and
`pages_read` with no way for a model to pick.

### The host appends

A post does **not** append. The crate's own rule is that *"a tool call is a
request to speak — the host appends, the host decides"*, and the host that
appends is the reply path that has always appended: it carries the folded steps,
the live SSE frame, the resolved mentions and the board-card correlation, none of
which a tool holds. So `desk_post` and `desk_close` are collected in
`delegation::TurnSpeech` and become the turn's reply.

`desk_dm` is the exception and journals directly, because a narrowed audience is
not something a turn's single reply can express.

### It never silences a turn

An agent that answers **without** calling a speech tool still has its return text
journaled, exactly as before. Going quiet because a model forgot a tool call is
not an acceptable failure mode, so it is not one this knob can produce. The
suppression is gated on `TurnSpeech::spoke()`, not on the manifest flag.

### Nothing here starts a turn

`desk_dm` is one agent addressing another, so this is where the agent-to-agent
edge stops being hypothetical. It stays an edge that **journals a row and runs
nothing**. That is the rule `CompanyEvent::AgentReply` already states about its
own `mentions`:

> never consulted by dispatch … an agent naming another agent draws a chip and
> files nothing to run. The edge does not exist, which is a stronger guarantee
> than an edge that is disabled.

and `mention_depth` beside it is the bound that would apply if it ever were. A
recipient hears about the row the next time it takes a turn, through its own
session delta — which is the stigmergic model, and needs no dispatch edge.

## Reading it back

`GET {scope}/agents/{agent_id}/session` — everything one agent said and heard,
in journal order, each row stamped with its channel. Projected through the
**same** `chat_history::agent_channels` the session itself uses, so the page
cannot claim an agent saw something it did not.

The operator sees more than the agent does, deliberately: the route projects
with the caller's own `Viewer`, and `Audience::admits` admits every operator
unconditionally. Privacy between agents is a deliberation device, never a
security boundary — see [`hivemind-asides.md`](hivemind-asides.md).

The console renders it as the **Session** tab on `#/team/<id>`
(`frontend/src/views/team/AgentSession.tsx`), reusing the room's own
`StepTimeline`, `ReferralConversation` and `AsideConversation` so an
agent-to-agent exchange reads the same way there as in the channel it happened
in.
