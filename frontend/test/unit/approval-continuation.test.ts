import { describe, expect, it } from "vitest";

import type { ApprovalSummary } from "@/api/types";
import { outstandingSignOffs } from "@/lib/approval-continuation";
import { approvedContinuation } from "@/lib/language";

/**
 * The confirmation after an approve says what is true (issue #561).
 *
 * Every surface answered an approve with *"Approved — the agent is completing
 * the action"*. The runtime has two states here and that sentence describes one
 * of them:
 *
 *  * the turn's **last** sign-off lands, the host runs the continuation, and the
 *    agent really has been asked to pick the work back up;
 *  * a sign-off lands while the same turn is still waiting on another, and the
 *    host banks the verdict and runs nothing at all (`still_waiting_report`,
 *    `src/company/runtime.rs`, issue #469).
 *
 * In the second state the sentence was a false claim with no recovery attached:
 * measured on staging, an operator watched it for four minutes while the step
 * trace still read *"Awaiting approval · didn't run"*, and the work only moved
 * after they happened to send an unrelated message.
 *
 * These tests pin the two halves apart, because the failure they guard is a
 * screen that looks completely normal while lying about whether anything is
 * running. The assertions are deliberately about the *claim* — "does this
 * sentence say work has started" — rather than about an exact string, so the
 * copy can be reworded without the guarantee quietly lapsing.
 */

const T0 = new Date("2026-08-14T09:00:00Z").getTime();

function approval(over: Partial<ApprovalSummary> & Pick<ApprovalSummary, "id">): ApprovalSummary {
  return {
    kind: "web_fetch",
    amount_usd: null,
    at_millis: T0,
    agent: "seo",
    thread: "desk-marketing",
    ...over,
  };
}

/** Whether a sentence claims the work is under way right now. */
function claimsUnderWay(line: string): boolean {
  return /\bnow\b/.test(line) || /is completing the action/.test(line);
}

const NOBODY_DECIDING = new Set<string>();

describe("what the console counts as still outstanding", () => {
  it("counts the turn's other undecided sign-offs", () => {
    const parked = [
      approval({ id: "a", batch: "turn-1" }),
      approval({ id: "b", batch: "turn-1" }),
      approval({ id: "c", batch: "turn-1" }),
    ];
    expect(outstandingSignOffs(parked[0], parked, NOBODY_DECIDING)).toBe(2);
  });

  it("is zero on the last one, which is the decision that releases the turn", () => {
    const parked = [approval({ id: "a", batch: "turn-1" })];
    expect(outstandingSignOffs(parked[0], parked, NOBODY_DECIDING)).toBe(0);
  });

  it("never counts an approval from a different turn", () => {
    // The whole point of keying on the host's batch: two unrelated parks sitting
    // in the queue together do not block each other's continuation, and telling
    // an operator to go and decide one would send them after the wrong row.
    const mine = approval({ id: "a", batch: "turn-1" });
    const theirs = approval({ id: "z", batch: "turn-2" });
    expect(outstandingSignOffs(mine, [mine, theirs], NOBODY_DECIDING)).toBe(0);
  });

  it("treats an ungated approval as outstanding-free", () => {
    // No `batch` means the host does not gate it (a workflow node, a scheduler
    // tick, a park journaled before #469) — it continues on its own decision.
    // Two of them are not a batch, and pairing them would invent a wait that
    // does not exist.
    const a = approval({ id: "a" });
    const b = approval({ id: "b" });
    expect(outstandingSignOffs(a, [a, b], NOBODY_DECIDING)).toBe(0);
  });

  it("does not count a sibling that is already being decided", () => {
    // The sentence tells the operator what is still waiting on *them*. A row
    // mid-resolve needs nothing further, and whichever lands last releases the
    // turn.
    const parked = [
      approval({ id: "a", batch: "turn-1" }),
      approval({ id: "b", batch: "turn-1" }),
    ];
    expect(outstandingSignOffs(parked[0], parked, new Set(["b"]))).toBe(0);
  });

  it("excludes the approval being decided even before it is marked in flight", () => {
    // A view that tracks in-flight ids in React state holds the pre-click value
    // in the handler's closure, so the id being decided is legitimately absent
    // from `deciding`. Counting it would make every single-item turn report one
    // outstanding sign-off and never claim to continue.
    const parked = [approval({ id: "a", batch: "turn-1" })];
    expect(outstandingSignOffs(parked[0], parked, NOBODY_DECIDING)).toBe(0);
  });
});

describe("what the console says an approve bought", () => {
  it("does not claim anything is running while the turn is still waiting", () => {
    const line = approvedContinuation(approval({ id: "a", batch: "turn-1" }), 2);
    expect(claimsUnderWay(line)).toBe(false);
    expect(line).toMatch(/^Approved/);
    // And it says what the operator has to do about it, which is the recovery
    // the old copy left them to guess at.
    expect(line).toMatch(/remaining 2 sign-offs/);
  });

  it("gets its grammar right for a single remaining sign-off", () => {
    const line = approvedContinuation(approval({ id: "a", batch: "turn-1" }), 1);
    expect(line).toMatch(/remaining 1 sign-off on this step is decided/);
    expect(line).not.toMatch(/sign-offs/);
  });

  it("says the agent has picked it back up once nothing is outstanding", () => {
    const line = approvedContinuation(approval({ id: "a", batch: "turn-1" }), 0);
    expect(claimsUnderWay(line)).toBe(true);
    // Named recovery. A released continuation still queues behind the runtime's
    // per-company serial lock for an unbounded time (issue #390) and the console
    // cannot see that wait, so "asked for" is all this may promise.
    expect(line).toMatch(/send it a message/i);
  });

  it("names no agent for an effect the runtime performs itself", () => {
    // Issue #395's distinction, kept: a park with no `agent` has nobody to
    // message, so it must neither name one nor offer that as the way out.
    const line = approvedContinuation(approval({ id: "a", agent: null }), 0);
    expect(line).not.toMatch(/agent/);
    expect(line).not.toMatch(/message/i);
    expect(claimsUnderWay(line)).toBe(true);
  });

  it("still holds off the claim for an agentless effect that is gated", () => {
    const line = approvedContinuation(approval({ id: "a", agent: null, batch: "turn-1" }), 1);
    expect(claimsUnderWay(line)).toBe(false);
    expect(line).not.toMatch(/agent/);
  });

  it("names the thing decided when the surface files the line into a transcript", () => {
    const line = approvedContinuation(approval({ id: "a" }), 0, "Fetch bbc.com");
    expect(line).toContain("Fetch bbc.com");
  });

  /**
   * The end-to-end shape of the defect, in one assertion: the Approvals page is
   * itemised, so approving one row of a three-call turn is exactly the case that
   * produced four minutes of silence under a sentence claiming completion.
   */
  it("approving one row of a three-call turn promises nothing is running", () => {
    const parked = [
      approval({ id: "a", batch: "turn-1" }),
      approval({ id: "b", batch: "turn-1" }),
      approval({ id: "c", batch: "turn-1" }),
    ];
    const line = approvedContinuation(
      parked[0],
      outstandingSignOffs(parked[0], parked, NOBODY_DECIDING),
      "Fetch bbc.com",
    );
    expect(claimsUnderWay(line)).toBe(false);
    expect(line).toMatch(/remaining 2 sign-offs on this step are decided/);
  });
});
