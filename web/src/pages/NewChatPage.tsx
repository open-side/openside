import { Layers } from "lucide-react";
import { useState } from "react";
import { useNavigate } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { Composer } from "../components/Composer";
import { ModelPicker } from "../components/ModelPicker";
import { setShareDraft, useShareDraft } from "../pwa/share";
import { useShell } from "../shell";
import { BrandMark, ICON_STROKE } from "../ui";

/**
 * `/`: an empty conversation; the first message creates the session and opens it. On wide
 * screens the greeting and composer sit together mid-screen; on phones the composer stays
 * at the thumb.
 */
export function NewChatPage() {
  const navigate = useNavigate();
  const shell = useShell();
  const [model, setModel] = useState("");
  const [isolated, setIsolated] = useState(false);
  const draft = useShareDraft();
  return (
    <main className="conversation new-chat">
      <AppHeader newChat={false} />
      <div className="new-chat-stage">
        <div className="empty-chat">
          <BrandMark size={36} />
          <h1>What’s on your mind?</h1>
          <p>Your agent runs on this computer.</p>
        </div>
        <Composer
          draft={draft}
          onDraftApplied={() => setShareDraft(null)}
          sessionId={null}
          model={model}
          browserSpace={isolated ? "isolated" : "shared"}
          tools={
            <>
              <ModelPicker value={model} onChange={setModel} />
              {/* Off by default: the shared profile is where the user's logins already are. */}
              <button
                type="button"
                className="tool-pill"
                aria-pressed={isolated}
                title="Browse with separate cookies and logins, wiped when the conversation is archived or deleted"
                onClick={() => setIsolated((on) => !on)}
              >
                <Layers size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                <span className="tool-pill-label">Isolated browser</span>
              </button>
            </>
          }
          onCreated={(id) => {
            shell.refreshSessions();
            navigate(`/sessions/${encodeURIComponent(id)}`);
          }}
        />
      </div>
    </main>
  );
}
