// Production web shell over the loopback host. Chrome desktop viewports only.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createRequire } from "node:module";
import { writeFixture } from "./web-fixture-data.mjs";

const { chromium } = createRequire(import.meta.url)("playwright-core");
const root = resolve(import.meta.dirname, "..");
const binary = join(root, "webhost/target/release/hghubx-web");
const webHtml = join(root, "dist/web.html");
const output = join(root, "test-results/web-player");
await mkdir(output, { recursive: true });

function start(args) {
  const child = spawn(binary, args, { cwd: root, stdio: ["ignore", "pipe", "pipe"] });
  let log = "";
  child.stdout.on("data", chunk => { log += chunk.toString(); });
  child.stderr.on("data", chunk => { log += chunk.toString(); });
  const ready = new Promise((resolveReady, reject) => {
    const timer = setTimeout(() => reject(new Error(`服务没有启动\n${log}`)), 8000);
    child.stdout.on("data", () => {
      const match = log.match(/ready http:\/\/127\.0\.0\.1:(\d+)/);
      if (!match) return;
      clearTimeout(timer);
      resolveReady(`http://127.0.0.1:${match[1]}`);
    });
    child.on("exit", code => {
      clearTimeout(timer);
      reject(new Error(`服务退出 ${code}\n${log}`));
    });
  });
  return { child, ready, stop: () => { child.kill("SIGTERM"); } };
}

let debugPage;
const browser = await chromium.launch({
  headless: true,
  executablePath: "/usr/local/bin/google-chrome",
  args: ["--autoplay-policy=no-user-gesture-required"],
});
const external = [];
const report = { browser: browser.version(), viewports: [], external, cases: [] };
let fixtureDir;
try {
  const plain = start(["--dist", join(root, "dist"), "--port", "0"]);
  const plainOrigin = await plain.ready;
  const context = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await context.route("**/*", route => {
    const url = new URL(route.request().url());
    if (url.origin === new URL(plainOrigin).origin) return route.continue();
    external.push(url.origin);
    return route.abort("blockedbyclient");
  });
  const page = await context.newPage();
  debugPage = page;
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  await page.goto(plainOrigin);
  await page.getByText("没有可用的播放后端", { exact: false }).waitFor();
  assert.equal(await page.getByText("离线示例甲").count(), 0);
  assert.deepEqual(errors, []);
  report.cases.push({ name: "unconfigured", origin: plainOrigin, errors });
  await context.close();
  plain.stop();

  fixtureDir = await mkdtemp(join(tmpdir(), "hghubx-web-test-"));
  await writeFixture(fixtureDir);
  const fixture = start(["--dist", join(root, "dist"), "--fixture", fixtureDir, "--port", "0"]);
  const origin = await fixture.ready;
  const session = await browser.newContext();
  await session.route("**/*", route => {
    const url = new URL(route.request().url());
    if (url.origin === new URL(origin).origin) return route.continue();
    external.push(url.origin);
    return route.abort("blockedbyclient");
  });
  const player = await session.newPage();
  debugPage = player;
  const playerErrors = [];
  player.on("pageerror", error => playerErrors.push(error.message));
  await player.setViewportSize({ width: 390, height: 844 });
  await player.goto(origin);
  await player.getByRole("button", { name: /离线示例甲/ }).click();
  await player.getByRole("button", { name: "第 2 集" }).click();
  await player.waitForFunction(() => {
    const video = document.querySelector("video");
    return !!video && video.readyState >= 2 && video.currentTime > 0 && !video.paused;
  });
  const narrowColumns = await player.locator(".app").evaluate(node => getComputedStyle(node).gridTemplateColumns);
  assert.equal(narrowColumns.includes(" "), false, `窄屏不应分成两列: ${narrowColumns}`);
  await player.screenshot({ path: join(output, "phone.png"), fullPage: true });
  await player.setViewportSize({ width: 1280, height: 800 });
  const wideColumns = await player.locator(".app").evaluate(node => getComputedStyle(node).gridTemplateColumns);
  assert.equal(wideColumns.includes(" "), true, `宽屏应分成两列: ${wideColumns}`);
  await player.screenshot({ path: join(output, "desktop.png"), fullPage: true });
  await player.getByRole("button", { name: "第 3 集" }).click();
  await player.getByRole("alert").filter({ hasText: "没有可播放的本地文件" }).waitFor();
  assert.equal(await player.locator("video").count(), 0);
  assert.deepEqual(playerErrors, []);
  assert.deepEqual(external, []);
  report.cases.push({
    name: "fixture-playback",
    origin,
    browser: browser.version(),
    viewports: ["390x844", "1280x800"],
    narrowColumns,
    wideColumns,
    note: "桌面 Chrome 视口，不是手机真机",
  });
  await session.close();
  fixture.stop();
  report.passed = true;
} catch (error) {
  report.passed = false;
  report.error = String(error.stack || error);
  const visible = await debugPage?.locator("body").innerText().catch(() => "");
  console.error(report.error);
  console.error("visible", visible);
  process.exitCode = 1;
} finally {
  await browser.close();
  if (fixtureDir) await rm(fixtureDir, { recursive: true, force: true });
  await writeFile(join(output, "report.json"), JSON.stringify(report, null, 2));
  console.log(JSON.stringify({ passed: report.passed, output, browser: report.browser }));
}
