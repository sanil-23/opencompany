import { expect, test, type Page } from "@playwright/test";

/**
 * Proof for the one-agent-one-session change: a teammate's session is reachable
 * against the live host, at an address.
 *
 * The claim spans the whole stack and no unit render can make it. The console
 * asks `GET {scope}/agents/{id}/session`; the host answers by resolving which
 * channels that agent can read — through the *same* function that decides the
 * agent's own context — and stamps every row with the channel it was said on.
 * A stubbed client proves the component; only a host proves the projection.
 *
 * The tab is an address (`#/team/<id>?tab=session`), so the walk is a deep link
 * rather than a click path: that is the property that lets one operator paste a
 * teammate's session to another, and it is the one a click path would not test.
 *
 * Either populated or empty is correct — whether the harness company's data
 * directory holds any transcript decides which. What must never happen is the
 * panel failing to render, or holding a spinner forever.
 */

/**
 * Clears both things that can stand in front of a deep link.
 *
 * "Skip setup" is the activation gate's control and "Skip for now" is the
 * guided tour's — two different surfaces with two different labels, and either
 * can be up on a fresh data directory. Both are tried because which one appears
 * depends on state this test does not own.
 */
async function dismissOnboarding(page: Page) {
  for (const name of ["Skip setup", "Skip for now"]) {
    const skip = page.getByRole("button", { name });
    for (let attempt = 0; attempt < 5; attempt += 1) {
      if (!(await skip.isVisible().catch(() => false))) break;
      await skip.click({ force: true }).catch(() => {});
      await page.waitForTimeout(300);
    }
  }
}

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const seen = JSON.stringify({ skipped: true, seenAt: Date.now() });
    for (const key of ["oc-tour:single", "oc-tour:e2e-harness-co", "oc-tour:null"]) {
      window.localStorage.setItem(key, seen);
    }
  });
});

test("a teammate's session opens from its own address", async ({ page }) => {
  // Clear the gate FIRST, on a neutral address. Dismissing it navigates, and a
  // navigation rewrites the hash — so opening the deep link before the gate is
  // gone would test the gate's redirect rather than the link.
  await page.goto("/");
  await dismissOnboarding(page);

  await page.goto("/#/team/engineer?tab=session");
  await dismissOnboarding(page);

  // The tab resolved from the hash rather than defaulting to Overview — the
  // whole point of `useHashTab`, and what makes the link shareable.
  const tab = page.getByRole("tab", { name: "Session" });
  await expect(tab).toBeVisible({ timeout: 30_000 });
  await expect(tab).toHaveAttribute("aria-selected", "true");

  // One of the three honest states, never a spinner that never settles: the
  // stream, the "nothing yet" line, or a host that keeps no session.
  const stream = page.getByTestId("agent-session");
  const settled = stream
    .or(page.getByText(/has not said or heard anything yet/))
    .or(page.getByText(/does not keep a per-agent session yet/));
  await expect(settled.first()).toBeVisible({ timeout: 30_000 });

  // If there are rows at all, every one of them says which channel it came
  // from. A merged stream whose rows do not is unreadable — two teammates
  // answering in two desks interleave with nothing to tell them apart.
  if (await stream.isVisible().catch(() => false)) {
    const rows = await page.getByTestId("agent-session-row").count();
    if (rows > 0) {
      expect(await page.getByTestId("agent-session-channel").count()).toBe(rows);
    }
  }
});
