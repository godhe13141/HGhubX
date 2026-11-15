// Explicit example host. This is not the normal browser startup and is not a real catalog.
import { spawn } from "node:child_process";
import { mkdtemp } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { writeFixture } from "./web-fixture-data.mjs";

const root = resolve(import.meta.dirname, "..");
const dir = await mkdtemp(join(tmpdir(), "hghubx-web-fixture-"));
await writeFixture(dir);
const child = spawn("cargo", [
  "run", "--quiet", "--manifest-path", join(root, "webhost/Cargo.toml"), "--release", "--",
  "--dist", join(root, "dist"),
  "--fixture", dir,
  "--port", process.env.HGHUBX_WEB_PORT || "17651",
], { cwd: root, stdio: "inherit", env: { ...process.env, RUSTUP_TOOLCHAIN: process.env.RUSTUP_TOOLCHAIN || "stable" } });
child.on("exit", code => process.exit(code ?? 1));
process.on("SIGINT", () => child.kill("SIGINT"));
process.on("SIGTERM", () => child.kill("SIGTERM"));
