import Hls from "hls.js";
import { useEffect, useRef, useState } from "react";

export function WebPlayer({ src }: { src: string }) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const [failure, setFailure] = useState<string | null>(null);

  useEffect(() => {
    const video = videoRef.current;
    if (!video) return;
    setFailure(null);
    let hls: Hls | null = null;
    const play = () => {
      void video.play().catch(() => setFailure("浏览器没有开始播放，可以点播放器上的播放按钮"));
    };
    if (Hls.isSupported()) {
      hls = new Hls({ enableWorker: false });
      hls.loadSource(src);
      hls.attachMedia(video);
      hls.on(Hls.Events.MANIFEST_PARSED, play);
      hls.on(Hls.Events.ERROR, (_event, data) => {
        if (data.fatal) setFailure("播放器无法读取这一集");
      });
    } else if (video.canPlayType("application/vnd.apple.mpegurl")) {
      video.src = src;
      video.addEventListener("loadedmetadata", play, { once: true });
    } else {
      setFailure("这个浏览器不能播放 HLS");
    }
    return () => {
      hls?.destroy();
      video.removeAttribute("src");
      video.load();
    };
  }, [src]);

  return (
    <div className="player">
      <video ref={videoRef} controls muted playsInline aria-label="播放画面" />
      {failure ? <p role="alert">{failure}</p> : <p className="status">正在播放同源示例媒体</p>}
    </div>
  );
}
