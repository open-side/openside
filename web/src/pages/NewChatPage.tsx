import { useState } from "react";
import { useNavigate } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { Composer } from "../components/Composer";
import { ModelPicker } from "../components/ModelPicker";
import { useShell } from "../shell";

/** `/`: an empty conversation; the first message creates the session and opens it. */
export function NewChatPage() {
  const navigate = useNavigate();
  const shell = useShell();
  const [model, setModel] = useState("");
  return (
    <main className="conversation new-chat">
      <AppHeader title="New chat" subtitle="openside" />
      <div className="conversation-scroll">
        <div className="empty-chat">
          <h2>What’s on your mind?</h2>
        </div>
      </div>
      <Composer
        sessionId={null}
        model={model}
        tools={<ModelPicker value={model} onChange={setModel} />}
        onCreated={(id) => {
          shell.refreshSessions();
          navigate(`/sessions/${encodeURIComponent(id)}`);
        }}
      />
    </main>
  );
}
