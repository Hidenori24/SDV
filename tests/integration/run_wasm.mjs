// Integration tests against the built sdv_core.wasm (the artifact that ships to the browser), without a browser.
//   node tests/integration/run_wasm.mjs [path/to/sdv_core.wasm]
// 1) the wasm must reproduce the C++ reference logs (golden) row by row
// 2) every scenario case in tests/integration/scenarios/*.json must satisfy its expectations
import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { load, evaluate } from "./lib.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "../..");
const wasmPath = process.argv[2] ?? join(root, "rust/sdv-core/target/wasm32-unknown-unknown/release/sdv_core.wasm");
const { run } = await load(wasmPath);

let failed = 0, passed = 0;
const report = (name, failures) => {
  if (failures.length === 0) { passed++; console.log(`ok    ${name}`); return; }
  failed++; console.log(`FAIL  ${name}`); for (const f of failures) console.log(`        ${f}`);
};

// ---- 1) golden parity: the same four runs as rust/sdv-core/tests/scenario.rs ----
const DEMO_STEPS = [{ until_s: 2.0, throttle: 0.6 }, { until_s: 5.0, throttle: 0.2, steer: 0.2 }, { until_s: 7.0, brake: 0.6 }];
const stall = (target, at_s) => ({ at_s, action: "stall", target });
const GOLDEN = [
  ["cpp_latest.csv", []],
  ["cpp_stall_brake2.csv", [stall("brake", 2.0)]],
  ["cpp_stall_steering3_brake3.5.csv", [stall("steering", 3.0), stall("brake", 3.5)]],
  ["cpp_stall_engine1_vd2.csv", [stall("engine", 1.0), stall("vehicledynamics", 2.0)]],
];
// golden column -> index in the wasm output buffer (see lib.mjs SIGNALS) and tolerance (same as golden_cpp.rs)
const COLS = [["t", 0, 2e-3], ["throttle", 7, 3e-3], ["brake", 8, 3e-3], ["steer", 9, 2e-4], ["drive_accel_cmd", 10, 2e-3], ["brake_decel_cmd", 11, 2e-3],
  ["steer_angle_cmd", 12, 2e-3], ["x", 1, 2e-3], ["y", 2, 2e-3], ["yaw", 3, 2e-3], ["v", 4, 2e-3], ["yaw_rate", 5, 2e-3], ["wheel_omega", 6, 2e-3],
  ["estop", 13, 2e-3], ["system_state", 14, 2e-3]];
for (const [file, events] of GOLDEN) {
  const lines = readFileSync(join(root, "rust/sdv-core/tests/golden", file), "utf8").trim().split("\n");
  const header = lines[0].split(",");
  const failures = [];
  if (header.join() !== COLS.map((c) => c[0]).join()) failures.push(`golden header changed: ${header}`);
  const rows = run({ name: file, steps: DEMO_STEPS, events }, lines.length - 1);
  lines.slice(1).forEach((line, i) => {
    const g = line.split(",").map(Number);
    COLS.forEach(([name, idx, tol], k) => {
      if (failures.length < 5 && !(Math.abs(g[k] - rows[i][idx]) <= tol)) failures.push(`row ${i} ${name}: golden=${g[k]} wasm=${rows[i][idx]}`);
    });
  });
  report(`wasm reproduces the C++ log: ${file} (${lines.length - 1} rows)`, failures);
}

// ---- 2) scenario cases with expectations ----
const dir = join(here, "scenarios");
for (const file of readdirSync(dir).filter((f) => f.endsWith(".json")).sort()) {
  const c = JSON.parse(readFileSync(join(dir, file), "utf8"));
  const rows = run(c.scenario, c.seconds, c.config);
  report(`scenario ${c.name} (${c.expect.length} checks)`, evaluate(rows, c.expect));
}

console.log(`\n${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
