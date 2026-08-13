import type { ApprovalSummary } from "@/api/types";

/**
 * How many sign-offs a turn is still blocked on once this one is decided
 * (issue #561).
 *
 * The runtime does not continue a turn per approval — it banks each verdict and
 * runs the continuation once, when the **last** call the turn parked has an
 * answer (`continue_turn`, `src/company/runtime.rs`, issue #469). So approving
 * one row of a turn that parked three starts nothing at all, and the console
 * has to be able to say so rather than claiming the work is under way.
 *
 * Lives here rather than inline in the view for the reason every other
 * derivation in `lib/` does: getting it wrong produces a screen that looks
 * completely normal while saying the wrong thing. Three decisions, each of
 * which is a way this could be quietly false:
 *
 * * **Keyed on `batch`, and only on `batch`.** That field is the host's own
 *   parking-turn key, not a console grouping — `src/runtime/types.rs` states it
 *   as a contract: "the batch a card consolidates is by construction the same
 *   batch the runtime continues exactly once". An approval with **no** batch is
 *   one the host does not gate at all (a workflow node, a scheduler tick, a park
 *   journaled before #469), so it is never outstanding and never has anything
 *   outstanding beside it. Zero, not "unknown".
 * * **Counted over what is still parked.** `/approvals` answers with exactly the
 *   undecided ones — the host drops an approval from the queue in the first step
 *   of resolving it — so the pending list *is* the outstanding set. Counting over
 *   a remembered full batch would keep naming rows that were signed off minutes
 *   ago.
 * * **A decision already in flight is not outstanding.** The sentence this feeds
 *   tells an operator what is still waiting on *them*, and a row someone has
 *   already clicked needs nothing further; whichever of them lands last releases
 *   the turn. Counting it would send an operator looking for a row that is
 *   mid-resolve.
 *
 * `deciding` is the ids currently being resolved — a `Set` or the view's
 * verdict-keyed `Map`, since only membership is read. `a` itself is excluded
 * whether or not it appears there: a caller that tracks in-flight state in React
 * state holds the pre-click value in its closure, so its own id may not be in
 * there yet.
 */
export function outstandingSignOffs(
  a: ApprovalSummary,
  parked: readonly ApprovalSummary[],
  deciding: { has(id: string): boolean },
): number {
  if (!a.batch) return 0;
  return parked.filter((o) => o.id !== a.id && o.batch === a.batch && !deciding.has(o.id)).length;
}
