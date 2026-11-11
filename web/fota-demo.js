"use strict";

const $ = (id) => document.getElementById(id);
const initialImages = {
  A: { version: "1.0.0", gain: 0.8, curve: "linear" },
  B: { version: "0.9.0", gain: 0.8, curve: "linear" },
};
let images = cloneImages(initialImages);
let activeSlot = "A";
let failedSlot = null;
let lastComparison = null;
let updating = false;
let drivePhase = "parked";
let driveIndex = 0;
let vehicleSpeed = 0;
let baselineTestDone = false;
let updateComplete = false;
let afterTestDone = false;
let baselineImage = null;
let beforePath = [];
let afterPath = [];
const DRIVE_SPEED = 1.3;
const DRIVE_WHEELBASE = 0.72;
const DRIVE_STEP = 0.025;
const DRIVE_DURATION = 5;

function cloneImages(source) {
  return Object.fromEntries(Object.entries(source).map(([slot, image]) => [slot, { ...image }]));
}

function selectedMode() {
  return document.querySelector('input[name="variant"]:checked').value;
}

function nextImage(mode) {
  const current = images[activeSlot];
  const [major, minor] = current.version.split(".").map(Number);
  return {
    version: `${major}.${minor + 1}.0`,
    gain: mode === "constant" ? Number((current.gain + 0.7).toFixed(2)) : current.gain,
    curve: mode === "logic" ? (current.curve === "linear" ? "peak" : "linear") : current.curve,
  };
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function steeringOutput(image, angle) {
  if (image.curve === "linear") return clamp(angle * image.gain, -25, 25);
  const magnitude = Math.abs(angle);
  const output = magnitude <= 3 ? magnitude * 0.25 : 0.75 + (magnitude - 3) * 1.85;
  return clamp(Math.sign(angle) * output, -25, 25);
}

function signed(value) {
  const rounded = Math.round(value * 10) / 10;
  return `${rounded > 0 ? "+" : ""}${rounded.toFixed(1)}°`;
}

function imageDescription(image) {
  return `v${image.version} · gain ${image.gain.toFixed(2)} · ${image.curve === "peak" ? "peak response" : "linear"}`;
}

function renderSlots(candidate = nextImage(selectedMode()), trialSlot = null) {
  for (const slot of ["A", "B"]) {
    const card = $(`slot${slot}`);
    const isActive = slot === activeSlot;
    card.classList.toggle("active", isActive);
    card.classList.toggle("trial", slot === trialSlot);
    card.classList.toggle("failed", slot === failedSlot);
    $(`state${slot}`).textContent = isActive ? "active" : slot === trialSlot ? "trial" : slot === failedSlot ? "failed" : "fallback";
    $(`image${slot}`).textContent = imageDescription(images[slot]);
  }
  $("activeSummary").textContent = `active slot: ${activeSlot} · v${images[activeSlot].version}`;
  $("targetSlot").textContent = `書込先: Slot ${activeSlot === "A" ? "B" : "A"}`;
  const from = lastComparison?.baseline ?? images[activeSlot];
  const shownCandidate = lastComparison?.candidate ?? candidate;
  const mode = lastComparison?.mode ?? selectedMode();
  $("packageChange").textContent = mode === "constant"
    ? `Steering gain: ${from.gain.toFixed(2)} → ${shownCandidate.gain.toFixed(2)}`
    : `Response logic: ${from.curve} → ${shownCandidate.curve}`;
}

function chartPath(image, xToSvg, yToSvg) {
  const points = [];
  for (let angle = -30; angle <= 30; angle += 1) {
    const output = steeringOutput(image, angle);
    points.push(`${angle === -30 ? "M" : "L"}${xToSvg(angle).toFixed(1)},${yToSvg(output).toFixed(1)}`);
  }
  return points.join(" ");
}

function renderChart() {
  const mode = selectedMode();
  const active = lastComparison?.baseline ?? images[activeSlot];
  const candidate = lastComparison?.candidate ?? nextImage(mode);
  const angle = Number($("inputAngle").value);
  const oldOutput = steeringOutput(active, angle);
  const newOutput = steeringOutput(candidate, angle);
  const left = 52, right = 18, top = 18, bottom = 36;
  const width = 640 - left - right, height = 260 - top - bottom;
  const xToSvg = (x) => left + ((x + 30) / 60) * width;
  const yToSvg = (y) => top + ((28 - y) / 56) * height;
  const grid = [-20, -10, 0, 10, 20].map((tick) => {
    const y = yToSvg(tick).toFixed(1);
    return `<line x1="${left}" y1="${y}" x2="${640 - right}" y2="${y}" stroke="var(--line)"/><text x="${left - 8}" y="${Number(y) + 4}" fill="var(--sub)" text-anchor="end" font-size="11">${tick}</text>`;
  }).join("");
  const xLabels = [-30, -15, 0, 15, 30].map((tick) => {
    const x = xToSvg(tick).toFixed(1);
    return `<line x1="${x}" y1="${top}" x2="${x}" y2="${260 - bottom}" stroke="var(--line)"/><text x="${x}" y="${250}" fill="var(--sub)" text-anchor="middle" font-size="11">${tick}°</text>`;
  }).join("");
  $("responseChart").innerHTML = `${grid}${xLabels}
    <path d="${chartPath(active, xToSvg, yToSvg)}" fill="none" stroke="#8792a5" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
    <path d="${chartPath(candidate, xToSvg, yToSvg)}" fill="none" stroke="var(--accent)" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"/>
    <circle cx="${xToSvg(angle).toFixed(1)}" cy="${yToSvg(oldOutput).toFixed(1)}" r="5" fill="#8792a5" stroke="var(--panel)" stroke-width="2"/>
    <circle cx="${xToSvg(angle).toFixed(1)}" cy="${yToSvg(newOutput).toFixed(1)}" r="5" fill="var(--accent)" stroke="var(--panel)" stroke-width="2"/>
    <text x="${left + width / 2}" y="258" fill="var(--sub)" text-anchor="middle" font-size="11">driver input</text>
    <text x="14" y="${top + height / 2}" fill="var(--sub)" text-anchor="middle" font-size="11" transform="rotate(-90 14 ${top + height / 2})">steering output</text>`;
  $("inputValue").textContent = `${angle > 0 ? "+" : ""}${angle}°`;
  $("oldOutput").textContent = signed(oldOutput);
  $("newOutput").textContent = signed(newOutput);
  $("outputDelta").textContent = signed(newOutput - oldOutput);
  $("responseChart").setAttribute("aria-label", `入力 ${angle}度。更新前 ${signed(oldOutput)}、更新案 ${signed(newOutput)}。横軸はdriver input、縦軸はsteering output。`);
  renderSlots(candidate);
}

function driverInputAt(time) {
  if (time < 1) return 0;
  if (time < 2) return 18;
  if (time < 3) return -18;
  if (time < 4) return 18;
  return 0;
}

function simulateDrive(image) {
  const path = [];
  let x = 0, y = 0, heading = 0;
  const sampleCount = Math.round(DRIVE_DURATION / DRIVE_STEP);
  for (let index = 0; index <= sampleCount; index += 1) {
    const time = index * DRIVE_STEP;
    const fraction = time / DRIVE_DURATION;
    const speed = fraction > 0.88 ? DRIVE_SPEED * (1 - fraction) / 0.12 : DRIVE_SPEED;
    const input = driverInputAt(time);
    const steer = steeringOutput(image, input);
    const yawRate = (speed / DRIVE_WHEELBASE) * Math.tan((steer * Math.PI) / 180);
    heading += yawRate * DRIVE_STEP;
    x += speed * Math.cos(heading) * DRIVE_STEP;
    y += speed * Math.sin(heading) * DRIVE_STEP;
    path.push({ x, y, heading, time, input, steer, speed });
  }
  return path;
}

function routeDeviation(first, second) {
  if (!first.length || !second.length) return 0;
  let max = 0;
  const count = Math.min(first.length, second.length);
  for (let index = 0; index < count; index += 1) {
    max = Math.max(max, Math.hypot(first[index].x - second[index].x, first[index].y - second[index].y));
  }
  return max;
}

function renderDriveControls() {
  const moving = drivePhase === "before" || drivePhase === "after";
  const busy = moving || updating;
  const speedKph = (vehicleSpeed * 3.6).toFixed(1);
  const message = updating
    ? "停車中 · FOTA更新中"
    : moving
      ? `走行中${vehicleSpeed < DRIVE_SPEED * 0.5 ? "・制動中" : drivePhase === "before" ? "（更新前）" : "（更新後）"}`
      : "停車中";
  $("driveState").textContent = message;
  $("driveSpeed").textContent = `${speedKph} km/h`;
  $("driveState").classList.toggle("moving", moving);
  $("driveBefore").disabled = busy || baselineTestDone;
  $("driveAfter").disabled = busy || !updateComplete || afterTestDone;
  $("runUpdate").disabled = busy || !baselineTestDone || drivePhase !== "parked" || updateComplete;
  $("resetDemo").disabled = busy;
  const modeLocked = busy;
  $("failHealth").disabled = modeLocked;
  document.querySelectorAll('input[name="variant"]').forEach((input) => { input.disabled = modeLocked; });
  if (afterPath.length) {
    $("routeDelta").textContent = `同じ入力での最大軌跡差: ${routeDeviation(beforePath, afterPath).toFixed(2)} m`;
  } else if (baselineTestDone) {
    $("routeDelta").textContent = "停車を確認しました。FOTAを開始できます。";
  } else {
    $("routeDelta").textContent = "まず更新前の走行をしてください";
  }
}

function cssValue(name) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function drawDriveScene() {
  const canvas = $("driveCanvas");
  const rect = canvas.getBoundingClientRect();
  if (!rect.width || !rect.height) return;
  const dpr = window.devicePixelRatio || 1;
  const pixelWidth = Math.round(rect.width * dpr);
  const pixelHeight = Math.round(rect.height * dpr);
  if (canvas.width !== pixelWidth || canvas.height !== pixelHeight) {
    canvas.width = pixelWidth;
    canvas.height = pixelHeight;
  }
  const ctx = canvas.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, rect.width, rect.height);
  ctx.fillStyle = cssValue("--bg");
  ctx.fillRect(0, 0, rect.width, rect.height);

  const fitPaths = [beforePath, afterPath].filter((path) => path.length);
  const allPoints = fitPaths.flat();
  const minX = allPoints.length ? Math.min(...allPoints.map((point) => point.x)) : -1;
  const maxX = allPoints.length ? Math.max(...allPoints.map((point) => point.x)) : 1;
  const minY = allPoints.length ? Math.min(...allPoints.map((point) => point.y)) : -1;
  const maxY = allPoints.length ? Math.max(...allPoints.map((point) => point.y)) : 1;
  const spanX = Math.max(1, maxX - minX);
  const spanY = Math.max(1, maxY - minY);
  const scale = Math.min((rect.width - 100) / spanX, (rect.height - 88) / spanY, 95);
  const centerX = (minX + maxX) / 2;
  const centerY = (minY + maxY) / 2;
  const toCanvas = (point) => ({
    x: rect.width / 2 + (point.x - centerX) * scale,
    y: rect.height / 2 - (point.y - centerY) * scale,
  });

  const drawPath = (path, count, color, dash, width) => {
    if (!path.length || count < 2) return;
    ctx.beginPath();
    path.slice(0, count).forEach((point, index) => {
      const screen = toCanvas(point);
      if (index === 0) ctx.moveTo(screen.x, screen.y);
      else ctx.lineTo(screen.x, screen.y);
    });
    ctx.strokeStyle = color;
    ctx.lineWidth = width;
    ctx.lineCap = "round";
    ctx.lineJoin = "round";
    ctx.setLineDash(dash);
    ctx.stroke();
    ctx.setLineDash([]);
  };

  const accent = cssValue("--accent");
  const muted = cssValue("--sub");
  const beforeCount = drivePhase === "before" ? Math.min(beforePath.length, driveIndex + 1) : beforePath.length;
  drawPath(beforePath, beforeCount, muted, [], 3);
  if (afterPath.length) {
    const afterCount = drivePhase === "after" ? Math.min(afterPath.length, driveIndex + 1) : afterPath.length;
    drawPath(afterPath, afterCount, accent, afterTestDone || drivePhase === "after" ? [] : [7, 6], 3);
  }

  const carPath = drivePhase === "after" && afterPath.length ? afterPath : beforePath;
  const carIndex = carPath.length
    ? movingIndex(carPath.length)
    : 0;
  const position = carPath[carIndex] || { x: 0, y: 0, heading: 0 };
  const car = toCanvas(position);
  ctx.save();
  ctx.translate(car.x, car.y);
  ctx.rotate(-position.heading);
  ctx.fillStyle = "#171c26";
  ctx.fillRect(-12, -16, 5, 12);
  ctx.fillRect(7, -16, 5, 12);
  ctx.fillRect(-12, 6, 5, 12);
  ctx.fillRect(7, 6, 5, 12);
  ctx.fillStyle = accent;
  ctx.beginPath();
  ctx.moveTo(0, -22);
  ctx.lineTo(11, -10);
  ctx.lineTo(10, 17);
  ctx.lineTo(-10, 17);
  ctx.lineTo(-11, -10);
  ctx.closePath();
  ctx.fill();
  ctx.fillStyle = "#ffffff";
  ctx.fillRect(-6, -9, 12, 9);
  ctx.restore();

  ctx.fillStyle = cssValue("--ink");
  ctx.font = "600 13px system-ui, sans-serif";
  ctx.fillText("同じDriver input: straight → right → left → right → straight", 14, 22);
  ctx.fillStyle = cssValue("--sub");
  ctx.font = "12px system-ui, sans-serif";
  ctx.fillText(`仮想車速 ${vehicleSpeed > 0 ? (vehicleSpeed * 3.6).toFixed(1) : "0.0"} km/h`, 14, rect.height - 14);
  if (updating) {
    ctx.fillStyle = "rgba(0,0,0,.48)";
    ctx.fillRect(0, 0, rect.width, rect.height);
    ctx.fillStyle = "#ffffff";
    ctx.font = "700 22px system-ui, sans-serif";
    ctx.textAlign = "center";
    ctx.fillText("PARKED · FOTA IN PROGRESS", rect.width / 2, rect.height / 2);
    ctx.textAlign = "start";
  }
}

function movingIndex(pathLength) {
  return drivePhase === "before" || drivePhase === "after"
    ? Math.min(driveIndex, pathLength - 1)
    : pathLength - 1;
}

async function runDriveTest(which) {
  if (updating || drivePhase !== "parked") return;
  const isBefore = which === "before";
  const image = { ...images[activeSlot] };
  const path = simulateDrive(image);
  if (isBefore) {
    baselineImage = image;
    beforePath = path;
    afterPath = [];
    baselineTestDone = false;
    updateComplete = false;
    afterTestDone = false;
  } else {
    afterPath = path;
    afterTestDone = false;
  }
  drivePhase = which;
  driveIndex = 0;
  vehicleSpeed = DRIVE_SPEED;
  renderDriveControls();
  drawDriveScene();
  const interval = 18;
  for (let index = 0; index < path.length; index += 1) {
    driveIndex = index;
    vehicleSpeed = path[index].speed;
    renderDriveControls();
    drawDriveScene();
    await pause(interval);
  }
  vehicleSpeed = 0;
  drivePhase = "parked";
  if (isBefore) {
    baselineTestDone = true;
    setStatus("更新前の走行が終了し、完全停止しました。内容を選び、停車中にFOTAできます。");
  } else {
    afterTestDone = true;
    const changed = baselineImage && imageDescription(baselineImage) !== imageDescription(image);
    const detail = changed
      ? `同一入力で最大 ${routeDeviation(beforePath, afterPath).toFixed(2)} m ずれました。`
      : "rollback後のactive imageなので更新前と同じ軌跡です。";
    setStatus(`再走行が終了し、完全停止しました。${detail}`, changed ? "ok" : "");
  }
  renderDriveControls();
  drawDriveScene();
}

function addLog(message) {
  const item = document.createElement("li");
  item.textContent = message;
  $("eventLog").append(item);
  $("eventLog").scrollTop = $("eventLog").scrollHeight;
}

function setProgress(percent, currentStep = -1, failedStep = -1) {
  $("progressFill").style.width = `${percent}%`;
  $("progress").setAttribute("aria-valuenow", String(percent));
  document.querySelectorAll(".flowstep").forEach((step, index) => {
    step.classList.toggle("done", index < currentStep || (failedStep >= 0 && index < failedStep) || (currentStep === -1 && percent === 100));
    step.classList.toggle("current", index === currentStep);
    step.classList.toggle("failed", index === failedStep);
  });
}

function setStatus(message, kind = "") {
  $("status").className = `status${kind ? ` ${kind}` : ""}`;
  $("status").textContent = message;
}

function setBusy(busy) {
  updating = busy;
  renderDriveControls();
  drawDriveScene();
}

const pause = (ms) => new Promise((resolve) => window.setTimeout(resolve, ms));

async function runUpdate() {
  if (updating || !baselineTestDone || drivePhase !== "parked" || updateComplete) return;
  setBusy(true);
  $("eventLog").replaceChildren();
  setProgress(0);
  lastComparison = null;
  renderChart();
  const oldActive = activeSlot;
  const baseline = { ...images[oldActive] };
  const targetSlot = oldActive === "A" ? "B" : "A";
  const mode = selectedMode();
  const candidate = nextImage(mode);
  const modeLabel = mode === "constant" ? "gain変更" : "steeringロジック変更";
  const steps = [
    ["署名検証: release keyでpackageを確認", 14],
    [`${modeLabel} packageをSlot ${targetSlot}へstaging`, 32],
    ["readback digestがmanifestと一致", 50],
    [`Slot ${targetSlot}をtrial起動（出力hold中）`, 67],
    ["Steering ECU healthを3 cycle確認", 86],
    [`trial metadataをcommitしてSlot ${targetSlot}をactiveに設定`, 100],
  ];

  try {
    for (let index = 0; index < steps.length; index += 1) {
      const [message, progress] = steps[index];
      setProgress(progress, index);
      setStatus(message);
      addLog(message);
      if (index === 1) {
        images[targetSlot] = candidate;
        failedSlot = null;
        renderSlots(candidate, targetSlot);
      }
      await pause(620);
      if (index === 4 && $("failHealth").checked) {
        failedSlot = targetSlot;
        lastComparison = { baseline, candidate: { ...candidate }, mode };
        updateComplete = true;
        afterPath = simulateDrive(images[activeSlot]);
        setProgress(progress, -1, index);
        addLog("health check failed; trial metadata cleared");
        addLog(`rollback: Slot ${oldActive} remains active`);
        setStatus(`health check失敗。Slot ${oldActive}のactive imageを維持し、candidateをrollbackしました。`, "fail");
        renderChart();
        renderDriveControls();
        drawDriveScene();
        return;
      }
      if (index === 5) {
        activeSlot = targetSlot;
        failedSlot = null;
        lastComparison = { baseline, candidate: { ...candidate }, mode };
        renderChart();
      }
    }

    setProgress(100, -1);
    updateComplete = true;
    afterPath = simulateDrive(images[activeSlot]);
    setStatus(`更新成功。Slot ${targetSlot} v${candidate.version}をactiveにcommitしました。旧slotはrollback用に残っています。`, "ok");
    renderChart();
    renderDriveControls();
    drawDriveScene();
  } finally {
    setBusy(false);
  }
}

function resetDemo() {
  if (updating || drivePhase === "before" || drivePhase === "after") return;
  images = cloneImages(initialImages);
  activeSlot = "A";
  failedSlot = null;
  lastComparison = null;
  drivePhase = "parked";
  driveIndex = 0;
  vehicleSpeed = 0;
  baselineTestDone = false;
  updateComplete = false;
  afterTestDone = false;
  baselineImage = null;
  beforePath = [];
  afterPath = [];
  document.querySelector('input[name="variant"][value="constant"]').checked = true;
  $("failHealth").checked = false;
  $("inputAngle").value = 18;
  $("eventLog").replaceChildren();
  setProgress(0);
  setStatus("先に「更新前のテスト走行」を行い、車両を停止させてください。");
  renderChart();
  renderDriveControls();
  drawDriveScene();
}

document.querySelectorAll('input[name="variant"]').forEach((input) => input.addEventListener("change", () => {
  lastComparison = null;
  renderChart();
}));
$("inputAngle").addEventListener("input", renderChart);
$("runUpdate").addEventListener("click", runUpdate);
$("driveBefore").addEventListener("click", () => runDriveTest("before"));
$("driveAfter").addEventListener("click", () => runDriveTest("after"));
$("resetDemo").addEventListener("click", resetDemo);
renderChart();
renderDriveControls();
drawDriveScene();
window.addEventListener("resize", drawDriveScene);
