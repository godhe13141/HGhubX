import type { Detail, Episode, Paged, PlayInfo, VideoCard } from "../types";

export type WebCard = Pick<VideoCard, "id" | "title" | "episode_count">;
export type WebEpisode = Pick<Episode, "ep_num" | "title" | "duration">;
export type WebDetail = Pick<Detail, "id" | "title" | "episode_count"> & { episodes: WebEpisode[] };
export type WebPlay = Pick<PlayInfo, "id" | "ep_num" | "title" | "duration" | "video_url">;

export type WebBoot =
  | { kind: "unconfigured"; message: string }
  | { kind: "fixture"; label: string };

export type Parse<T> = { ok: true; value: T } | { ok: false; message: string };

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function numberField(record: Record<string, unknown>, key: string): number | null {
  const value = record[key];
  return typeof value === "number" && Number.isInteger(value) && value >= 0 ? value : null;
}

function textField(record: Record<string, unknown>, key: string): string | null {
  const value = record[key];
  return typeof value === "string" && value.trim().length > 0 ? value : null;
}

export function parseBoot(value: unknown): Parse<WebBoot> {
  if (!isRecord(value)) return { ok: false, message: "启动响应不是对象" };
  if (value.kind === "unconfigured") {
    const message = textField(value, "message");
    return message ? { ok: true, value: { kind: "unconfigured", message } } : { ok: false, message: "缺少未配置说明" };
  }
  if (value.kind === "fixture") {
    const label = textField(value, "label");
    return label ? { ok: true, value: { kind: "fixture", label } } : { ok: false, message: "缺少示例标记" };
  }
  return { ok: false, message: "无法识别启动状态" };
}

function parseCard(value: unknown): WebCard | null {
  if (!isRecord(value)) return null;
  const id = numberField(value, "id");
  const title = textField(value, "title");
  const episodeCount = numberField(value, "episode_count");
  if (id === null || id < 1 || title === null || episodeCount === null) return null;
  return { id, title, episode_count: episodeCount };
}

export function parseFeed(value: unknown): Parse<Paged<WebCard>> {
  if (!isRecord(value) || !Array.isArray(value.items) || !isRecord(value.pagination)) {
    return { ok: false, message: "列表响应不完整" };
  }
  const items: WebCard[] = [];
  for (const item of value.items) {
    const card = parseCard(item);
    if (!card) return { ok: false, message: "列表里有无法识别的条目" };
    items.push(card);
  }
  const page = numberField(value.pagination, "page");
  const pages = numberField(value.pagination, "pages");
  const size = numberField(value.pagination, "size");
  const total = numberField(value.pagination, "total");
  if (page === null || pages === null || size === null || total === null) {
    return { ok: false, message: "列表分页无法识别" };
  }
  return { ok: true, value: { items, pagination: { page, pages, size, total } } };
}

function parseEpisode(value: unknown): WebEpisode | null {
  if (!isRecord(value)) return null;
  const epNum = numberField(value, "ep_num");
  const title = textField(value, "title");
  const duration = numberField(value, "duration");
  if (epNum === null || epNum < 1 || title === null || duration === null) return null;
  return { ep_num: epNum, title, duration };
}

export function parseDetail(value: unknown): Parse<WebDetail> {
  if (!isRecord(value) || !Array.isArray(value.episodes)) return { ok: false, message: "详情响应不完整" };
  const id = numberField(value, "id");
  const title = textField(value, "title");
  const episodeCount = numberField(value, "episode_count");
  if (id === null || title === null || episodeCount === null) return { ok: false, message: "详情缺少标题或集数" };
  const episodes: WebEpisode[] = [];
  for (const item of value.episodes) {
    const episode = parseEpisode(item);
    if (!episode) return { ok: false, message: "选集里有无法识别的集" };
    episodes.push(episode);
  }
  return { ok: true, value: { id, title, episode_count: episodeCount, episodes } };
}

export function isSameOriginMedia(url: string): boolean {
  return url.startsWith("/hg/media/") && !url.includes("..") && !url.includes("://") && !url.includes("\\");
}

export function parsePlay(value: unknown): Parse<WebPlay> {
  if (!isRecord(value)) return { ok: false, message: "播放响应不是对象" };
  const id = numberField(value, "id");
  const epNum = numberField(value, "ep_num");
  const title = textField(value, "title");
  const duration = numberField(value, "duration");
  const videoUrl = textField(value, "video_url");
  if (id === null || epNum === null || title === null || duration === null || videoUrl === null) {
    return { ok: false, message: "播放响应缺少地址" };
  }
  if (!isSameOriginMedia(videoUrl)) return { ok: false, message: "播放地址超出本机同源范围" };
  return { ok: true, value: { id, ep_num: epNum, title, duration, video_url: videoUrl } };
}

export function parseError(value: unknown): string | null {
  if (!isRecord(value) || value.kind !== "error") return null;
  return textField(value, "message");
}
