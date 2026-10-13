import { CodeXml, Download, File, FileCode2, FileImage, FileText, Files } from "lucide-react";
import { useEffect, useState } from "react";
import { isOutputArtifact } from "../api/artifacts";
import { ApiError, type Client } from "../api/client";
import type { Artifact } from "../api/types";
import { useClient } from "../context";
import { Button, Dialog, ICON_STROKE, IconButton, useToast } from "../ui";
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

const basename = (path: string) => path.split("/").pop() ?? path;

const ORIGIN: Record<Artifact["source"], string> = {
  attachment: "Attached",
  tool: "Created",
  visual: "Visual",
  transcript: "Transcript",
};

function FileIcon({ mime }: { mime: string }) {
  const Icon = raster(mime) ? FileImage : /html|xml|svg|javascript|json|typescript/.test(mime) ? FileCode2 : textual(mime) ? FileText : File;
  return <Icon size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />;
}

async function download(client: Client, sessionId: string, path: string) {
  const blob = await client.artifactBlob(sessionId, path);
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = basename(path);
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

const failure = (cause: unknown) => (cause instanceof ApiError ? cause.message : "Request failed");

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
      .catch((cause) => !cancelled && setError(failure(cause)));
    return () => {
      cancelled = true;
    };
  }, [client, sessionId, artifact.path, showSource, source]);

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
          <Button
            variant="ghost"
            size="compact"
            onClick={() => void download(client, sessionId, artifact.path).catch((cause) => setError(failure(cause)))}
          >
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

/**
 * Files the session produced or received: one pill in the composer's tool row that opens a
 * sheet listing them (open to preview, or download). Nothing is added to the transcript.
 */
export function FilesTray({ sessionId, artifacts }: { sessionId: string; artifacts: Artifact[] }) {
  const client = useClient();
  const toast = useToast();
  const [open, setOpen] = useState(false);
  const [selected, setSelected] = useState<string | null>(null);
  const files = artifacts.filter((artifact) => artifact.source !== "transcript" && isOutputArtifact(artifact));
  if (!files.length) return null;
  const current = files.find((artifact) => artifact.path === selected);
  const close = () => {
    setOpen(false);
    setSelected(null);
  };
  const count = `${files.length} ${files.length === 1 ? "file" : "files"}`;
  return (
    <>
      <button type="button" className="tool-pill files-pill" aria-haspopup="dialog" onClick={() => setOpen(true)}>
        <Files size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span className="tool-pill-label">{count}</span>
      </button>
      <Dialog
        open={open}
        onClose={close}
        size="wide"
        className="tray-sheet"
        title={current ? basename(current.path) : "Files"}
        description={current ? undefined : "From this conversation"}
        back={current ? { label: "Back to files", onClick: () => setSelected(null) } : undefined}
      >
        <div className="tray-scroll thin-scroll">
          {current ? (
            <ArtifactPreview key={current.path} sessionId={sessionId} artifact={current} />
          ) : (
            <ul className="files-list" aria-label="Files">
              {files.map((artifact) => (
                <li key={artifact.path}>
                  <button type="button" className="files-open" title={artifact.path} onClick={() => setSelected(artifact.path)}>
                    <FileIcon mime={artifact.mime} />
                    <span className="files-text">
                      <span className="files-name">{basename(artifact.path)}</span>
                      <span className="files-meta">
                        {size(artifact.size)} · {ORIGIN[artifact.source]}
                      </span>
                    </span>
                  </button>
                  <IconButton
                    label={`Download ${basename(artifact.path)}`}
                    size="small"
                    className="files-download"
                    onClick={() => void download(client, sessionId, artifact.path).catch((cause) => toast.notify(failure(cause), "error"))}
                  >
                    <Download size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                </li>
              ))}
            </ul>
          )}
        </div>
      </Dialog>
    </>
  );
}
