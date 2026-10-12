import { CodeXml, Download, File, FileCode2, FileImage, FileText } from "lucide-react";
import { useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { Artifact } from "../api/types";
import { useClient } from "../context";
import { Button, ICON_STROKE } from "../ui";
import { ArtifactImage } from "./ArtifactImage";
import { SandboxFrame } from "./VisualFrame";
import "./browser.css";

export const TEXT_LIMIT = 200_000;

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const textual = (mime: string) =>
  mime.startsWith("text/") || /json|xml|svg|javascript|toml|yaml/.test(mime);

const raster = (mime: string) => mime.startsWith("image/") && mime !== "image/svg+xml";

function FileIcon({ mime }: { mime: string }) {
  const Icon = raster(mime) ? FileImage : /html|xml|svg|javascript|json|typescript/.test(mime) ? FileCode2 : textual(mime) ? FileText : File;
  return <Icon size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}

function ArtifactPreview({ sessionId, artifact }: { sessionId: string; artifact: Artifact }) {
  const client = useClient();
  const [source, setSource] = useState<{ text: string; total: number } | null>(null);
  const [showSource, setShowSource] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!showSource || source !== null) return;
    let cancelled = false;
    client
      .artifactBlob(sessionId, artifact.path)
      .then(async (blob) => ({ text: await blob.slice(0, TEXT_LIMIT).text(), total: blob.size }))
      .then((loaded) => !cancelled && setSource(loaded))
      .catch((cause) => !cancelled && setError(cause instanceof ApiError ? cause.message : "Request failed"));
    return () => {
      cancelled = true;
    };
  }, [client, sessionId, artifact.path, showSource, source]);

  async function download() {
    try {
      const blob = await client.artifactBlob(sessionId, artifact.path);
      const url = URL.createObjectURL(blob);
      const link = document.createElement("a");
      link.href = url;
      link.download = artifact.path.split("/").pop() ?? artifact.path;
      link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Request failed");
    }
  }

  return (
    <div className="artifact-preview">
      <div className="artifact-preview-header">
        <span className="artifact-preview-path" title={artifact.path}>
          {artifact.path}
        </span>
        <div className="artifact-preview-actions">
          {textual(artifact.mime) ? (
            <Button variant="ghost" size="compact" aria-pressed={showSource} onClick={() => setShowSource(!showSource)}>
              <CodeXml size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {showSource ? "Preview" : "Source"}
            </Button>
          ) : null}
          <Button variant="ghost" size="compact" onClick={() => void download()}>
            <Download size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
            Download
          </Button>
        </div>
      </div>
      {showSource ? (
        <>
          {source && source.total > TEXT_LIMIT ? (
            <p className="artifact-note" role="note">
              Showing the first {TEXT_LIMIT.toLocaleString("en-US")} of {source.total.toLocaleString("en-US")} bytes.
              Download the file for the rest.
            </p>
          ) : null}
          <pre className="visual-source" tabIndex={0}>
            {source?.text ?? "Loading…"}
          </pre>
        </>
      ) : raster(artifact.mime) ? (
        <ArtifactImage sessionId={sessionId} path={artifact.path} alt={artifact.path} />
      ) : artifact.preview_url ? (
        <SandboxFrame src={artifact.preview_url} title={`Preview of ${artifact.path}`} />
      ) : (
        <p className="artifact-empty">No preview for this file type.</p>
      )}
      {error ? (
        <p role="alert" className="artifact-error">
          {error}
        </p>
      ) : null}
    </div>
  );
}

/** Files the session produced or received, with a sandboxed preview of the selected one. */
export function ArtifactsPanel({ sessionId, artifacts }: { sessionId: string; artifacts: Artifact[] }) {
  const [selected, setSelected] = useState<string | null>(null);
  const visible = artifacts.filter((artifact) => artifact.source !== "transcript");
  if (!visible.length) return null;
  const current = visible.find((artifact) => artifact.path === selected);
  return (
    <section className="artifacts" aria-label="Files">
      <header className="artifacts-header">
        <div>
          <h3 className="artifacts-title">Files</h3>
          <p className="artifacts-subtitle">
            {visible.length} {visible.length === 1 ? "file" : "files"} from this conversation
          </p>
        </div>
      </header>
      <ul className="artifact-list">
        {visible.map((artifact) => (
          <li key={artifact.path}>
            <button
              type="button"
              className="artifact-row"
              aria-pressed={artifact.path === selected}
              onClick={() => setSelected(artifact.path === selected ? null : artifact.path)}
            >
              <FileIcon mime={artifact.mime} />
              <span className="artifact-text">
                <span className="artifact-path">{artifact.path}</span>
                <span className="artifact-meta">
                  {artifact.source} · {size(artifact.size)}
                </span>
              </span>
            </button>
          </li>
        ))}
      </ul>
      {current ? <ArtifactPreview key={current.path} sessionId={sessionId} artifact={current} /> : null}
    </section>
  );
}
