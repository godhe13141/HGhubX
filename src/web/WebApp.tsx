import { useCallback, useEffect, useState } from "react";
import { loadBoot, loadDetail, loadFeed, loadPlay, type WebCard } from "./client";
import type { WebDetail } from "./parse";
import { WebPlayer } from "./WebPlayer";

type Screen =
  | { kind: "booting" }
  | { kind: "unconfigured"; message: string }
  | { kind: "broken"; message: string }
  | { kind: "catalog"; label: string; items: WebCard[]; detail: DetailState };

type DetailState =
  | { kind: "idle" }
  | { kind: "loading"; id: number }
  | { kind: "failed"; id: number; message: string }
  | { kind: "open"; detail: WebDetail; episode: EpisodeState };

type EpisodeState =
  | { kind: "idle" }
  | { kind: "loading"; episode: number }
  | { kind: "failed"; episode: number; message: string }
  | { kind: "playing"; episode: number; mediaUrl: string };

function statusText(screen: Screen): string {
  switch (screen.kind) {
    case "booting":
      return "正在连接本机播放服务";
    case "unconfigured":
      return screen.message;
    case "broken":
      return screen.message;
    case "catalog":
      switch (screen.detail.kind) {
        case "idle":
          return "选择一部示例，再选择集数";
        case "loading":
          return "正在读取选集";
        case "failed":
          return screen.detail.message;
        case "open":
          switch (screen.detail.episode.kind) {
            case "idle":
              return `${screen.detail.detail.title} · 请选择集数`;
            case "loading":
              return `正在准备第 ${screen.detail.episode.episode} 集`;
            case "failed":
              return screen.detail.episode.message;
            case "playing":
              return `${screen.detail.detail.title} · 第 ${screen.detail.episode.episode} 集`;
            default: {
              const exhaustive: never = screen.detail.episode;
              return exhaustive;
            }
          }
        default: {
          const exhaustive: never = screen.detail;
          return exhaustive;
        }
      }
    default: {
      const exhaustive: never = screen;
      return exhaustive;
    }
  }
}

export default function WebApp() {
  const [screen, setScreen] = useState<Screen>({ kind: "booting" });

  const boot = useCallback(async () => {
    setScreen({ kind: "booting" });
    const started = await loadBoot();
    if (!started.ok) {
      setScreen({ kind: "broken", message: started.message });
      return;
    }
    if (started.value.kind === "unconfigured") {
      setScreen({ kind: "unconfigured", message: started.value.message });
      return;
    }
    const feed = await loadFeed();
    if (!feed.ok) {
      setScreen({ kind: "broken", message: feed.message });
      return;
    }
    setScreen({ kind: "catalog", label: started.value.label, items: feed.value.items, detail: { kind: "idle" } });
  }, []);

  useEffect(() => { void boot(); }, [boot]);

  const openTitle = async (id: number) => {
    setScreen(current => current.kind === "catalog" ? { ...current, detail: { kind: "loading", id } } : current);
    const detail = await loadDetail(id);
    setScreen(current => {
      if (current.kind !== "catalog") return current;
      if (!detail.ok) return { ...current, detail: { kind: "failed", id, message: detail.message } };
      return { ...current, detail: { kind: "open", detail: detail.value, episode: { kind: "idle" } } };
    });
  };

  const openEpisode = async (id: number, episode: number) => {
    setScreen(current => {
      if (current.kind !== "catalog" || current.detail.kind !== "open") return current;
      return { ...current, detail: { ...current.detail, episode: { kind: "loading", episode } } };
    });
    const play = await loadPlay(id, episode);
    setScreen(current => {
      if (current.kind !== "catalog" || current.detail.kind !== "open") return current;
      if (!play.ok) return { ...current, detail: { ...current.detail, episode: { kind: "failed", episode, message: play.message } } };
      return { ...current, detail: { ...current.detail, episode: { kind: "playing", episode, mediaUrl: play.value.video_url } } };
    });
  };

  const open = screen.kind === "catalog" && screen.detail.kind === "open" ? screen.detail : null;
  const mediaUrl = open && open.episode.kind === "playing" ? open.episode.mediaUrl : null;

  return (
    <main className="page">
      <header className="top">
        <h1>HGhubX 浏览器</h1>
        <p>手机和桌面浏览器共用的播放页。这里不登录，也不读取真实剧库。</p>
      </header>
      {screen.kind === "catalog" ? <p className="banner" role="status">{screen.label}</p> : null}
      <p className="status" role="status" aria-live="polite">{statusText(screen)}</p>
      {screen.kind === "broken" || screen.kind === "unconfigured" ? (
        <button type="button" onClick={() => void boot()}>重新检查</button>
      ) : null}
      {screen.kind === "catalog" ? (
        <div className="app">
          <section aria-label="列表">
            <h2>列表</h2>
            {screen.items.length === 0 ? <p>列表现在是空的。</p> : (
              <ul>
                {screen.items.map(item => (
                  <li key={item.id}>
                    <button type="button" aria-current={open?.detail.id === item.id ? "true" : undefined} onClick={() => void openTitle(item.id)}>
                      {item.title}
                      <small>{item.episode_count} 集</small>
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </section>
          <section aria-label="播放">
            <h2>选集与播放</h2>
            {screen.detail.kind === "failed" ? <p role="alert">{screen.detail.message}</p> : null}
            {open ? (
              <>
                <div className="episodes" role="group" aria-label="选集">
                  {open.detail.episodes.map(episode => (
                    <button
                      key={episode.ep_num}
                      type="button"
                      aria-current={open.episode.kind !== "idle" && open.episode.episode === episode.ep_num ? "true" : undefined}
                      onClick={() => void openEpisode(open.detail.id, episode.ep_num)}
                    >
                      第 {episode.ep_num} 集
                    </button>
                  ))}
                </div>
                {open.episode.kind === "failed" ? <p role="alert">{open.episode.message}</p> : null}
                {mediaUrl ? <WebPlayer src={mediaUrl} /> : <p className="stage">选择一集后在这里播放。</p>}
              </>
            ) : <p className="stage">{screen.detail.kind === "loading" ? "正在读取选集" : "还没有选择内容。"}</p>}
          </section>
        </div>
      ) : null}
    </main>
  );
}
