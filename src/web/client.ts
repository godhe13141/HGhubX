import { parseBoot, parseDetail, parseError, parseFeed, parsePlay, type Parse, type WebBoot, type WebCard, type WebDetail, type WebPlay } from "./parse";
import type { Paged } from "../types";

export type WebResult<T> = { ok: true; value: T } | { ok: false; message: string };

async function readJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

async function getParsed<T>(path: string, parse: (value: unknown) => Parse<T>): Promise<WebResult<T>> {
  let response: Response;
  try {
    response = await fetch(path, { headers: { Accept: "application/json" } });
  } catch {
    return { ok: false, message: "无法连接本机播放服务" };
  }
  const body = await readJson(response);
  if (!response.ok) {
    return { ok: false, message: parseError(body) ?? `请求失败（HTTP ${response.status}）` };
  }
  const parsed = parse(body);
  return parsed.ok ? { ok: true, value: parsed.value } : { ok: false, message: parsed.message };
}

export function loadBoot(): Promise<WebResult<WebBoot>> {
  return getParsed("/hg/boot", parseBoot);
}

export function loadFeed(): Promise<WebResult<Paged<WebCard>>> {
  return getParsed("/hg/feed", parseFeed);
}

export function loadDetail(id: number): Promise<WebResult<WebDetail>> {
  return getParsed(`/hg/videos/${id}`, parseDetail);
}

export function loadPlay(id: number, episode: number): Promise<WebResult<WebPlay>> {
  return getParsed(`/hg/videos/${id}/episodes/${episode}`, parsePlay);
}

export type { WebCard };
