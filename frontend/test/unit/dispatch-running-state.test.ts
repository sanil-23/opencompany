import { describe, expect, it } from "vitest";

import { dispatchThreadKey } from "@/lib/chat";

/**
 * The shell holds in-flight dispatches as `taskId -> threadKey`, and the
 * shape is the point (tinysweeper, #2369).
 *
 * A per-thread **count** is decremented by whichever terminal arrives, so an
 * unrelated card completing in the same thread takes a live attempt's row
 * down, and that attempt's own completion then finds nothing to clear. Keying
 * by task id means a completion can only ever clear the attempt it belongs to.
 *
 * These exercise the same reducers the shell applies, kept here because the
 * console's unit runner is for pure functions.
 */
const started = (
  running: Record<string, string>,
  taskId: string,
  chatId: string,
  parentId?: string,
): Record<string, string> => ({
  ...running,
  [taskId]: dispatchThreadKey(chatId, parentId),
});

const completed = (
  running: Record<string, string>,
  taskId: string,
): Record<string, string> => {
  if (!(taskId in running)) return running;
  const next = { ...running };
  delete next[taskId];
  return next;
};

const waitingOn = (running: Record<string, string>, key: string): boolean =>
  Object.values(running).includes(key);

describe("in-flight dispatch marks", () => {
  it("clears only the attempt that finished", () => {
    let running = started({}, "task-a", "main", "50");
    running = started(running, "task-b", "main", "50");

    // B finishes first. A is still running and must keep its row.
    running = completed(running, "task-b");

    expect(waitingOn(running, dispatchThreadKey("main", "50"))).toBe(true);
    expect(Object.keys(running)).toEqual(["task-a"]);
  });

  it("a completion for an untracked card changes nothing", () => {
    // A board-created card raises no mark, so its terminal must not remove
    // somebody else's.
    const running = started({}, "task-a", "main", "50");

    expect(completed(running, "task-from-the-board")).toBe(running);
  });

  it("is empty once every attempt has reported", () => {
    let running = started({}, "task-a", "main");
    running = completed(running, "task-a");

    expect(waitingOn(running, dispatchThreadKey("main"))).toBe(false);
  });

  it("keeps a thread's work apart from its channel's", () => {
    const running = started({}, "task-a", "main", "50");

    expect(waitingOn(running, dispatchThreadKey("main", "50"))).toBe(true);
    expect(waitingOn(running, dispatchThreadKey("main"))).toBe(false);
  });
});
