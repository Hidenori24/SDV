// Smoke tests for the browser GUI. Only what needs a real browser: the logic is covered by tests/integration.
import { test, before, after } from "node:test";
import assert from "node:assert/strict";
import { startServer, launchBrowser, openPage, text, setSpeed } from "./helpers.mjs";

let server, browser;
before(async () => { server = await startServer(); browser = await launchBrowser(); });
after(async () => { await browser?.close(); await server?.close(); });

async function withPage(path, fn) {
  const s = await openPage(browser, server.url + path);
  try { await fn(s); assert.deepEqual(s.errors, [], "no console or page errors"); } finally { await s.close(); }
}

test("the simulator starts: the wasm loads and time advances", () =>
  withPage("", async ({ page }) => {
    await page.waitForFunction(() => parseFloat(document.getElementById("s_t").textContent) > 0.3, null, { timeout: 10_000 });
    assert.equal(await page.locator("#err").isVisible(), false, "no load error banner");
  }));

test("config: a valid config is saved, an invalid one is rejected with the reason, and the previous one comes back", () =>
  withPage("", async ({ page }) => {
    await page.click("#cfgbox summary");
    const set = async (id, value) => { await page.fill(id, String(value)); await page.dispatchEvent(id, "input"); };

    await set("#f_tuning_safety_heartbeat_timeout_ms", 300);
    await page.click("#cfgSave");
    assert.match(await text(page, "#nvmStatus"), /Active: 正常 \(seq 1\)/);

    await set("#f_vehicle_spec_max_accel_mps2", 99);
    await page.click("#cfgSave");
    const rejected = await text(page, "#cfgMsg");
    assert.match(rejected, /拒否しました/);
    assert.match(rejected, /vehicle_spec\.max_accel_mps2/, "the reason names the field");
    assert.match(await text(page, "#nvmStatus"), /Active: 正常 \(seq 1\)/, "nothing was stored");

    await set("#f_vehicle_spec_max_accel_mps2", 3);
    await page.click("#cfgSave");
    assert.match(await text(page, "#nvmStatus"), /Active: 正常 \(seq 2\)/);

    await page.click("#cfgRollback");
    assert.equal(await page.inputValue("#f_vehicle_spec_max_accel_mps2"), "2", "back to the default acceleration");
    assert.equal(await page.inputValue("#f_tuning_safety_heartbeat_timeout_ms"), "300", "the first config is the one that came back");
  }));

test("fault injection: two stopped ECUs end in an E-STOP and the car stops", () =>
  withPage("", async ({ page }) => {
    await page.click("#scnbox summary");
    await page.click('.scnpreset[data-p="fault"]');
    await page.click("#scnPlay");
    await setSpeed(page, 4);
    await page.waitForFunction(() => document.getElementById("safety").className.includes("estop"), null, { timeout: 20_000 });
    await page.waitForFunction(() => parseFloat(document.getElementById("s_v").textContent) === 0, null, { timeout: 20_000 });
  }));

test("steering sign: left makes the yaw rate positive, right negative", () =>
  withPage("", async ({ page }) => {
    for (const [key, sign] of [["ArrowLeft", 1], ["ArrowRight", -1]]) {
      await page.reload();
      await page.keyboard.down("ArrowUp");
      await page.waitForFunction(() => parseFloat(document.getElementById("s_v").textContent) > 1.0, null, { timeout: 15_000 });
      await page.keyboard.down(key);
      await page.waitForFunction((s) => parseFloat(document.getElementById("s_yr").textContent) * s > 5, sign, { timeout: 10_000 });
      await page.keyboard.up(key);
      await page.keyboard.up("ArrowUp");
    }
  }));

test("FOTA demo: drive, update while parked, drive the same course again", () =>
  withPage("fota-demo.html", async ({ page }) => {
    await page.click("#driveBefore");
    await page.waitForSelector("#runUpdate:not([disabled])", { timeout: 60_000 });
    assert.match(await text(page, "#driveState"), /停車中/);
    await page.click("#runUpdate");
    await page.waitForSelector("#driveAfter:not([disabled])", { timeout: 60_000 });
    assert.match(await text(page, "#status"), /更新成功/);
    assert.match(await text(page, "#activeSummary"), /active slot: B/);
    await page.click("#driveAfter");
    await page.waitForFunction(() => /最大軌跡差/.test(document.getElementById("routeDelta").textContent), null, { timeout: 60_000 });
  }));
