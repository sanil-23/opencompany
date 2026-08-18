# OpenHuman Module

OpenHuman is the tenant harness, embedded as a **library**. The
`src/harness/` module links `openhuman_core` (`vendor/openhuman`) directly and,
under `feature = "openhuman"`, builds one openhuman `Agent` per manifest
`[[agent]]` through `AgentBuilder`. The default build links none of it and
keeps its offline, echo-brained behaviour.

The builder seams are wired to OpenCompany's own ports:

- **Persona** → each agent gets a system prompt framing it as its manifest
  `role` at the company, built with `SystemPromptBuilder::for_subagent` and
  `omit_identity` so it speaks as that role rather than openhuman's own
  assistant identity.
- **Memory** → `harness::memory::OcMemory`, an openhuman `Memory` over the
  OpenCompany `ContextStore`.
- **Inference provider** → `harness::provider::HostedProvider`, an
  OpenAI-compatible client for the hosted TinyHumans brain (`chat()` sends the
  full history and parses token/cost usage back out), with a `MockProvider`
  for offline tests.
- **Approval policy** → `harness::policy::ApprovalPolicy` maps `[policy].mode`
  onto openhuman's `ToolPolicy`; the security-tier words
  (readonly/supervised/full) line up 1:1. See
  [approval parking](#approval-parking) for how a gated call reaches the
  operator.
- **Tools / skills** → injected from the company's manifest grants.

See [`docs/modules/runtime/README.md`](../runtime/README.md) for `HarnessPool`
and [`docs/spec/integrations/openhuman.md`](../../spec/integrations/openhuman.md)
for the full integration contract.

## `HarnessBrain` — cognition on the embedded runtime

`harness::brain::HarnessBrain` implements the `Brain` cognition port over a
`HarnessPool`: each operator message runs one openhuman agent turn and returns
the agent's reply, in place of the offline `EchoBrain`'s `"You said: …"`. A
company routes through it when the `RuntimeBuilder` has both a harness pool
(`with_harness`) and any inference source that resolves at build time, and no
explicit brain — brain precedence is `with_brain` > harness > hosted/echo. The
`opencompany` binary's `attach_harness` resolves the managed default from the
environment (below).

Which brain a company runs is chosen once, when its runtime is built. A company
that resolved **no** inference source at boot is on the offline echo brain and
stays there for as long as that runtime lives, no matter what the console saves
afterwards — a company runtime is built once and cached in the
`CompanyRegistry`. That transition is reported honestly as `restartRequired`
(issue #266) and cleared by rebuilding the runtime in place (issue #290, see
[`docs/spec/runtime/rebuild.md`](../../spec/runtime/rebuild.md)) rather than by
a process restart.

Everything *after* that first transition is live: once a company is on the
harness path, `TenantProvider` re-resolves the effective config — console
runtime override > manifest `[inference]` > managed env default — on every turn,
so a provider switch or key rotation reaches agents on the next turn with no
rebuild at all.

## The per-turn step budget (issue #926)

An agent turn is bounded by openhuman's `AgentConfig::max_tool_iterations` — the
number of **model calls** one turn may make, defaulting to 10. It is not a
deadline and not a sub-agent budget, and it is not the step count the console
shows: a console "step" is one tool call or one coalesced thinking run
(`harness::steps::fold_steps`), so a turn stopped at 10 model calls routinely
renders as ~20 steps. `harness::workflow_build` is the one place OpenCompany
overrides the cap (`set_max_tool_iterations(7)`); everything else inherits the
default.

**Hitting the cap is invisible in the reply, by construction.** openhuman does
not error. It asks the model for a resumable checkpoint with tools disabled
(`MAX_ITER_CHECKPOINT_INSTRUCTION`) and returns that prose through the same
`Ok(String)` a finished turn returns. The instruction asks for "Done so far" and
"Next steps" and never asks the model to mention the limit, so a capped turn
typically ends on a confident plan — indistinguishable from an agent that
finished and described what it would do. Only openhuman's deterministic
*fallback* checkpoint (used when the model's wrap-up call fails or comes back
empty) names the cap.

So the fact is carried out of band. `Agent::last_turn_hit_cap()` is read in
`CompanyAgent::run_with_steer` while the agent lock is still held — the same
place and for the same reason as `last_turn_usage` — and becomes
`TurnOutcome::exhausted_budget: Option<ExhaustedStepBudget>`. The cap it quotes
comes off the agent that enforced it, never a constant on this side, because
callers may override it per agent.

`harness::brain` renders it as **its own operator bubble**, not appended to the
reply: the reply is the agent's answer and this is the system saying the agent
was cut off. The wording deliberately mirrors
`DrainedRequests::overflow_notice` (issue #561) — same "Heads up:" opening, same
habit of quoting the limit that actually applied, same closing move of telling
the operator the one thing they can do. Two systems reporting silently-dropped
work in two different voices is how one of them stops being believed.

The cap itself is **not** raised. Raising it moves the cliff and keeps the
silence, which is the failure this fixes.

## Approval parking

openhuman resolves a `ToolPolicyDecision::RequireApproval` **inline**: it blocks
the tool call and feeds the model a refusal, then lets the turn continue. That
refusal used to be the only trace a gated call left — nothing was written to
OpenCompany's `ApprovalGate` or its journal, so the operator's Approvals page
stayed empty however many tools an agent parked (issue #172).

The two halves are now joined:

1. `ApprovalPolicy::check` projects every `RequireApproval` onto an `Effect`
   (`effect_for` — the tool name becomes the effect `kind`, the arguments become
   the payload) and pushes it onto the shared `ApprovalRequestQueue` carried on
   `HarnessDeps`. Duplicates (a model re-trying the same blocked call) collapse.
2. `HarnessBrain::run_cycle` clears that queue before its turns and drains it
   after, parking each request through `CycleHost::park_effect` — capped at
   `MAX_APPROVAL_REQUESTS_PER_TURN`.

`park_effect` is deliberately **not** `emit_effect`: the verdict was already
reached inside the turn, and re-evaluating it against the coarser `ApprovalGate`
taxonomy would `Allow` (and so "execute" as a no-op) anything in the `Other`
group — which is most gated tool calls — making the request vanish again.

**Not yet wired: resume-after-approval.** Because openhuman resolves the decision
inline, approving a parked tool call records the verdict and clears the queue but
does not re-dispatch the tool; the operator re-asks. Suspending and resuming a
call inside openhuman's session loop is separate work.

## Inference config (environment)

`harness::provider::harness_inference_from_env` resolves the endpoint, key, and
default model, most specific first:

| Value | Source | Fallback |
| --- | --- | --- |
| key | `OPENCOMPANY_INFERENCE_KEY` | `TINYHUMANS_API_KEY` — **no key ⇒ echo brain** |
| url | `OPENCOMPANY_INFERENCE_URL` | `https://api.tinyhumans.ai/openai/v1` |
| model | `OPENCOMPANY_INFERENCE_MODEL` | `chat-v1` |

The two key names keep a per-tenant override distinct from the platform-wide
credential the hosting manager injects.

This is the **lowest**-precedence source. A company's own key, set write-only
through the console (`PUT …/inference` with `key`, stored under the
`inference/key` secret), wins over both env names — including on the `managed`
provider, where only the credential changes and the platform endpoint is kept.
Clearing it (`PUT …/inference` with `key: ""`, the console's **Remove key**)
falls back to the env credential rather than 401ing.

## Cost metering

`harness::cost` maps a completed turn's usage onto the ledger and the
`UsageMeter`. `HarnessPool::run` reads the real per-turn token/cost totals from
openhuman's public `Agent::last_turn_usage()` accessor
(tinyhumansai/openhuman#4940), so metering is **live**. Gating differs by
surface: a usage sample is recorded whenever tokens moved (the `/openai/v1`
passthrough reports tokens but bills backend-side, echoing no USD), while a
ledger `inference.spend` entry is written only when the turn actually cost USD —
so a token-bearing zero-cost turn meters usage without a `$0.00` spend line. An
offline provider that reports no usage yields a zero turn, which writes nothing.

## `src/openhuman/` — legacy JSON-RPC path (behind `openhuman-rpc`)

The former out-of-process seam is retained for one release and then removed.
`src/openhuman/` still hosts the launcher (`opencompany open-human
[--mode core|desktop] [--release] [--dry-run]` — Core shells out through Cargo
to `openhuman-core`, Desktop calls `cargo tauri dev`/`build` directly and ports
OpenHuman's `dev:app`/`dev:wry`/`macos:build:release`/`tauri:build:ui` preflight
into Rust: vendored CEF-aware `tauri-cli` install, `CEF_PATH`, `.env` load
(seeded from `.env.example` only in Desktop mode when absent), and macOS
keychain + signing) and the JSON-RPC adapters —
`rpc.rs` (the `OpenHumanRpc` transport trait + `MockOpenHumanRpc`),
`http_client.rs` (the `reqwest` client behind `openhuman-rpc`), `tools.rs`
(`OpenHumanToolProvider`, catalog filtered by manifest grants, ungranted calls
rejected), and `channel.rs` (`OpenHumanChannelAdapter`). It degrades to
built-in tools and the operator channel with a boot warning when OpenHuman is
unreachable — never a boot failure. New work targets the embedded library, not
this path.
