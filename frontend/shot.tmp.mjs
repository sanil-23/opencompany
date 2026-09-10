import { chromium } from "@playwright/test";

const OUT = process.env.OUT;
const browser = await chromium.launch();
const ctx = await browser.newContext({ viewport: { width: 1440, height: 1000 }, deviceScaleFactor: 2 });
const page = await ctx.newPage();
page.on("console", (m) => { if (m.type() === "error") console.log("[console error]", m.text()); });
page.on("pageerror", (e) => console.log("[pageerror]", e.message));

await page.goto("http://localhost:5173/#/finances/invoicing", { waitUntil: "networkidle" });
// Dismiss the first-run tour if it opens over the console.
const skip = page.getByRole("button", { name: "Skip for now" });
await skip.waitFor({ state: "visible", timeout: 5000 }).then(() => skip.click()).catch(() => {});
await page.waitForTimeout(2500);
console.log("URL:", page.url());
console.log("pitch present:", await page.getByTestId("chargebee-pitch").count());
console.log("offer state:", await page.getByTestId("chargebee-offer").getAttribute("data-offer-state").catch(() => "none"));
await page.screenshot({ path: `${OUT}/invoicing-light.png`, fullPage: true });

await page.emulateMedia({ colorScheme: "dark" });
await page.waitForTimeout(600);
await page.screenshot({ path: `${OUT}/invoicing-dark.png`, fullPage: true });

await browser.close();
