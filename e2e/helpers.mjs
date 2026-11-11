// Shared by the end-to-end tests: a static server for web/ and a browser.
import { chromium } from "playwright-core";
import { createServer } from "node:http";
import { readFile, access } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { extname, join, normalize } from "node:path";

const WEB = fileURLToPath(new URL("../web/", import.meta.url));
const TYPES = { ".html": "text/html; charset=utf-8", ".js": "text/javascript; charset=utf-8", ".css": "text/css", ".json": "application/json", ".wasm": "application/wasm", ".svg": "image/svg+xml", ".png": "image/png" };

/** Serve web/ on a free port. The tests need web/sdv_core.wasm (build it and copy it there; see e2e/README.md). */
export async function startServer() {
  await access(join(WEB, "sdv_core.wasm")).catch(() => {
    throw new Error("web/sdv_core.wasm is missing: build it (cargo build --release --target wasm32-unknown-unknown --manifest-path rust/sdv-core/Cargo.toml) and copy it to web/");
  });
  const server = createServer(async (req, res) => {
    const path = normalize(decodeURIComponent(new URL(req.url, "http://x").pathname)).replace(/^([/\\])+/, "");
    const file = join(WEB, path === "" ? "index.html" : path);
    if (!file.startsWith(WEB)) { res.writeHead(403).end(); return; } // never leave web/
    try {
      const body = await readFile(file);
      res.writeHead(200, { "content-type": TYPES[extname(file)] || "application/octet-stream" }).end(body);
    } catch { res.writeHead(404).end("not found"); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { url: `http://127.0.0.1:${server.address().port}/`, close: () => new Promise((resolve) => server.close(resolve)) };
}

/** CI uses the Chrome of the runner image; locally set E2E_BROWSER_PATH to any Chromium binary. */
export function launchBrowser() {
  const executablePath = process.env.E2E_BROWSER_PATH;
  return chromium.launch(executablePath ? { executablePath } : { channel: "chrome" });
}

/** A fresh page (own localStorage) that remembers console and page errors. */
export async function openPage(browser, url) {
  const context = await browser.newContext({ viewport: { width: 1200, height: 1700 } });
  const page = await context.newPage();
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  // the browser asks for /favicon.ico by itself; the page has none
  page.on("console", (m) => { if (m.type() === "error" && !/favicon\.ico/.test(m.location().url ?? "")) errors.push(m.text()); });
  await page.goto(url);
  return { page, errors, close: () => context.close() };
}

export const text = async (page, selector) => (await page.locator(selector).innerText()).replace(/\s+/g, " ").trim();

/** Playback speed of the simulator (the slider), to keep the tests short. */
export const setSpeed = (page, speed) => page.evaluate((v) => { const s = document.getElementById("spd"); s.value = v; s.dispatchEvent(new Event("input")); }, speed);
