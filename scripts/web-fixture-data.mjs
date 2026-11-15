import { execFileSync } from "node:child_process";
import { copyFile, mkdir, writeFile } from "node:fs/promises";
import { join } from "node:path";

export const FIXTURE_LABEL = "离线示例，不是真实剧库";

export async function writeFixture(dir) {
  await mkdir(dir, { recursive: true });
  execFileSync("ffmpeg", [
    "-hide_banner", "-loglevel", "error",
    "-f", "lavfi", "-i", "testsrc2=size=160x160:rate=10",
    "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100",
    "-t", "8",
    "-c:v", "libx264", "-preset", "ultrafast", "-g", "10", "-pix_fmt", "yuv420p",
    "-c:a", "aac",
    "-f", "hls", "-hls_time", "2", "-hls_playlist_type", "vod",
    "-hls_segment_filename", join(dir, "seg%03d.ts"),
    join(dir, "list.m3u8"),
  ], { timeout: 30000 });
  await copyFile(join(dir, "list.m3u8"), join(dir, "ep1.m3u8"));
  await copyFile(join(dir, "list.m3u8"), join(dir, "ep2.m3u8"));
  const episode = (ep, file) => ({ ep_num: ep, title: `第${ep}集`, duration: 8, file });
  const catalog = {
    label: FIXTURE_LABEL,
    items: [
      { id: 1, title: "离线示例甲", episodes: [episode(1, "ep1.m3u8"), episode(2, "ep2.m3u8"), episode(3, "missing.m3u8")] },
      { id: 2, title: "离线示例乙", episodes: [episode(1, "ep1.m3u8")] },
    ],
  };
  await writeFile(join(dir, "catalog.json"), JSON.stringify(catalog));
}
