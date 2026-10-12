import { useEffect, useState } from "react";
import { useClient } from "../context";
import { ImageViewer } from "./ImageViewer";
import "./browser.css";

/**
 * An image stored as a session artifact (attachments included). The raw route needs the
 * bearer token, so the bytes are fetched and shown from a blob URL.
 */
export function ArtifactImage({ sessionId, path, alt }: { sessionId: string; path: string; alt: string }) {
  const client = useClient();
  const [src, setSrc] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);
  const [zoomed, setZoomed] = useState(false);

  useEffect(() => {
    let url: string | null = null;
    let cancelled = false;
    client
      .artifactBlob(sessionId, path)
      .then((blob) => {
        if (cancelled) return;
        url = URL.createObjectURL(blob);
        setSrc(url);
      })
      .catch(() => !cancelled && setFailed(true));
    return () => {
      cancelled = true;
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, sessionId, path]);

  if (failed) return <span className="chip">{alt} (unavailable)</span>;
  if (!src) return <span className="chip">{alt}</span>;
  return (
    <>
      <button type="button" className="image-button" onClick={() => setZoomed(true)} aria-label={`Open image ${alt}`}>
        <img src={src} alt={alt} className="attachment-image" />
      </button>
      {zoomed ? <ImageViewer label={`Image: ${alt}`} title={alt} src={src} alt={alt} onClose={() => setZoomed(false)} /> : null}
    </>
  );
}
