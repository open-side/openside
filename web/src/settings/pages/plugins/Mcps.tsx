import { Box, Globe, PenTool, Pencil, Plus, RefreshCw, SquareTerminal, Trash2, X, type LucideIcon } from "lucide-react";
import { useCallback, useId, useState, type FormEvent, type ReactNode } from "react";
import type { McpServer, McpServerList, McpServerTransport, UpdateMcpServerRequest } from "../../../api/types";
import { useClient } from "../../../context";
import { errorMessage, useResource } from "../../../hooks/useResource";
import { t } from "../../../i18n";
import { ErrorNote, ResourceView, RowMenu, useConfirm } from "../../../pages/blocks";
import { Button, Dialog, DialogActions, ICON_STROKE, IconButton, InfoBanner, MenuItem, Segmented, Switch, useToast } from "../../../ui";
import { joinCommand, splitCommand } from "./command";
import { PluginGroup, PluginIcon, type TabFrame } from "./parts";
import { MCP_PRESETS, type McpPreset } from "./presets";

const MENU_ICON = { size: 18, strokeWidth: ICON_STROKE } as const;
const PRESET_ICONS: Record<string, LucideIcon> = { figma: PenTool, blender: Box };

function statusText(server: McpServer): string {
  switch (server.status) {
    case "connected":
      return t("plugins.mcp.status.connected", { count: server.tools.length });
    case "connecting":
      return t("plugins.mcp.status.connecting");
    case "vault_locked":
      return t("plugins.mcp.status.vaultLocked");
    case "failed":
      return server.error ? t("plugins.mcp.status.failed", { error: server.error }) : t("plugins.mcp.status.failedPlain");
    case "disabled":
      return t("plugins.mcp.status.disabled");
  }
}

interface SecretDraft {
  key: number;
  name: string;
  value: string;
  /** Already stored in the Vault: the value field replaces it only when typed into. */
  kept: boolean;
}

/** What the Connect form starts from: an empty form, a preset, or a saved server. */
interface ServerDraft {
  label: string;
  transport: McpServerTransport;
  command: string;
  cwd: string;
  url: string;
  secrets: { name: string; value: string; kept: boolean }[];
  privateNetwork: boolean;
}

const EMPTY_DRAFT: ServerDraft = { label: "", transport: "stdio", command: "", cwd: "", url: "", secrets: [], privateNetwork: false };

function presetDraft(preset: McpPreset): ServerDraft {
  return {
    label: t(preset.title),
    transport: preset.transport,
    command: preset.command ? joinCommand(preset.command) : "",
    cwd: "",
    url: preset.url ?? "",
    secrets: [],
    privateNetwork: false,
  };
}

function serverDraft(server: McpServer): ServerDraft {
  return {
    label: server.label,
    transport: server.transport,
    command: server.command ? joinCommand(server.command) : "",
    cwd: server.cwd ?? "",
    url: server.url ?? "",
    secrets: server.secrets.map((name) => ({ name, value: "", kept: true })),
    privateNetwork: server.allow_private_network,
  };
}

let nextKey = 0;

/**
 * Connect (create) or edit one server. The transport is fixed once saved: the daemon's PATCH has
 * no transport field. Secret values are write-only; a saved secret left blank stays as it is.
 */
function ServerForm({ draft, editing, onSaved, onCancel }: { draft: ServerDraft; editing: McpServer | null; onSaved: (server: McpServer) => void; onCancel: () => void }) {
  const client = useClient();
  const id = useId();
  const [label, setLabel] = useState(draft.label);
  const [transport, setTransport] = useState<McpServerTransport>(draft.transport);
  const [command, setCommand] = useState(draft.command);
  const [cwd, setCwd] = useState(draft.cwd);
  const [url, setUrl] = useState(draft.url);
  const [privateNetwork, setPrivateNetwork] = useState(draft.privateNetwork);
  const [privateConfirm, confirmPrivate] = useConfirm();
  const [secrets, setSecrets] = useState<SecretDraft[]>(() => draft.secrets.map((secret) => ({ ...secret, key: nextKey++ })));
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const argv = transport === "stdio" ? splitCommand(command) : null;
  const commandBroken = transport === "stdio" && command.trim() !== "" && argv === null;
  const ready = label.trim() !== "" && (transport === "stdio" ? Boolean(argv?.length) : url.trim() !== "");

  function setSecret(key: number, patch: Partial<SecretDraft>) {
    setSecrets((current) => current.map((secret) => (secret.key === key ? { ...secret, ...patch } : secret)));
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!ready) return;
    setBusy(true);
    setError(null);
    const stdio = transport === "stdio";
    try {
      let saved: McpServer;
      if (editing) {
        const changes: Record<string, string | null> = {};
        const present = new Set<string>();
        for (const secret of secrets) {
          const name = secret.name.trim();
          if (!name) continue;
          present.add(name);
          if (secret.value !== "") changes[name] = secret.value;
        }
        for (const name of editing.secrets) if (!present.has(name)) changes[name] = null;
        const patch: UpdateMcpServerRequest = { label: label.trim() };
        if (stdio) {
          patch.command = argv;
          // An empty string clears a saved working folder.
          patch.cwd = cwd.trim();
        } else {
          patch.url = url.trim();
          patch.allow_private_network = privateNetwork;
        }
        if (Object.keys(changes).length) patch.secrets = changes;
        saved = await client.updateMcpServer(editing.id, patch);
      } else {
        const values: Record<string, string> = {};
        for (const secret of secrets) if (secret.name.trim()) values[secret.name.trim()] = secret.value;
        saved = await client.createMcpServer({
          id: null,
          label: label.trim(),
          enabled: null,
          transport,
          command: stdio ? argv : null,
          cwd: stdio && cwd.trim() ? cwd.trim() : null,
          url: stdio ? null : url.trim(),
          // Only http servers connect over a network; the daemon refuses `true` on stdio.
          allow_private_network: !stdio && privateNetwork,
          secrets: values,
        });
      }
      onSaved(saved);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const stdio = transport === "stdio";
  return (
    <form className="dialog-form plugins-form" onSubmit={submit} aria-label={editing ? t("plugins.mcp.editTitle") : t("plugins.mcp.connectTitle")}>
      <label className="field">
        <span>{t("plugins.mcp.name")}</span>
        <input value={label} maxLength={80} onChange={(e) => setLabel(e.target.value)} required />
      </label>
      {editing ? null : (
        <div className="field">
          <span aria-hidden="true">{t("plugins.mcp.transport")}</span>
          <Segmented
            label={t("plugins.mcp.transport")}
            value={transport}
            onChange={setTransport}
            options={[
              { value: "stdio", label: t("plugins.mcp.transport.stdio") },
              { value: "http", label: t("plugins.mcp.transport.http") },
            ]}
          />
        </div>
      )}
      {stdio ? (
        <>
          <div className="field">
            <label htmlFor={`${id}-command-input`}>{t("plugins.mcp.command")}</label>
            <input
              id={`${id}-command-input`}
              className="plugins-mono"
              value={command}
              spellCheck={false}
              autoCapitalize="off"
              placeholder="npx -y @scope/server"
              aria-invalid={commandBroken || undefined}
              aria-describedby={`${id}-command`}
              onChange={(e) => setCommand(e.target.value)}
            />
            <span className="plugins-hint" id={`${id}-command`}>
              {commandBroken ? t("plugins.mcp.commandInvalid") : t("plugins.mcp.commandHint")}
            </span>
          </div>
          {argv && argv.length > 1 ? (
            <ol className="plugins-argv">
              {argv.map((arg, index) => (
                <li key={index}>{arg}</li>
              ))}
            </ol>
          ) : null}
          <div className="field">
            <label htmlFor={`${id}-cwd-input`}>{t("plugins.mcp.cwd")}</label>
            <input id={`${id}-cwd-input`} className="plugins-mono" value={cwd} spellCheck={false} autoCapitalize="off" aria-describedby={`${id}-cwd`} onChange={(e) => setCwd(e.target.value)} />
            <span className="plugins-hint" id={`${id}-cwd`}>
              {t("plugins.mcp.cwdHint")}
            </span>
          </div>
        </>
      ) : (
        <div className="field">
          <label htmlFor={`${id}-url-input`}>{t("plugins.mcp.url")}</label>
          <input
            id={`${id}-url-input`}
            className="plugins-mono"
            type="url"
            value={url}
            spellCheck={false}
            autoCapitalize="off"
            placeholder="https://"
            aria-describedby={`${id}-url`}
            onChange={(e) => setUrl(e.target.value)}
          />
          <span className="plugins-hint" id={`${id}-url`}>
            {t("plugins.mcp.urlHint")}
          </span>
        </div>
      )}
      {stdio ? null : (
        <div className="plugins-toggle">
          <span className="plugins-toggle-copy">
            <span id={`${id}-private`}>{t("plugins.mcp.privateNetwork")}</span>
            <span className="plugins-hint" id={`${id}-private-hint`}>
              {t("plugins.mcp.privateNetworkHint")}
            </span>
          </span>
          <Switch
            aria-labelledby={`${id}-private`}
            aria-describedby={`${id}-private-hint`}
            checked={privateNetwork}
            onChange={(next) => {
              // Opting in hands the server's secrets to whatever answers on the local network, so it
              // takes an explicit confirmation; opting out never does.
              if (!next) setPrivateNetwork(false);
              else
                confirmPrivate({
                  title: t("plugins.mcp.privateNetworkConfirmTitle"),
                  description: t("plugins.mcp.privateNetworkConfirmText"),
                  confirmLabel: t("plugins.mcp.privateNetworkConfirm"),
                  onConfirm: () => setPrivateNetwork(true),
                });
            }}
          />
          {privateConfirm}
        </div>
      )}
      <fieldset className="plugins-secrets">
        <legend>{stdio ? t("plugins.mcp.env") : t("plugins.mcp.headers")}</legend>
        <p className="plugins-hint">{t("plugins.mcp.secretsHint")}</p>
        {secrets.map((secret, index) => (
          <div className="plugins-secret" key={secret.key}>
            <input
              className="plugins-mono"
              value={secret.name}
              spellCheck={false}
              autoCapitalize="off"
              readOnly={secret.kept}
              placeholder={stdio ? "API_KEY" : "Authorization"}
              aria-label={t("plugins.mcp.secretName", { index: index + 1 })}
              onChange={(e) => setSecret(secret.key, { name: e.target.value })}
            />
            <input
              className="plugins-mono"
              type="password"
              autoComplete="new-password"
              value={secret.value}
              placeholder={secret.kept ? t("plugins.mcp.secretKept") : undefined}
              aria-label={t("plugins.mcp.secretValue", { index: index + 1 })}
              onChange={(e) => setSecret(secret.key, { value: e.target.value })}
            />
            <IconButton
              size="small"
              label={t("plugins.mcp.secretRemove", { name: secret.name || String(index + 1) })}
              onClick={() => setSecrets((current) => current.filter((other) => other.key !== secret.key))}
            >
              <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            </IconButton>
          </div>
        ))}
        <Button
          size="small"
          onClick={() => setSecrets((current) => [...current, { key: nextKey++, name: "", value: "", kept: false }])}
        >
          <Plus size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
          {stdio ? t("plugins.mcp.addEnv") : t("plugins.mcp.addHeader")}
        </Button>
      </fieldset>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button variant="ghost" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" disabled={busy || !ready}>
          {editing ? t("common.save") : t("plugins.mcp.connect")}
        </Button>
      </DialogActions>
    </form>
  );
}

function ServerRow({ server, controls }: { server: McpServer; controls: ReactNode }) {
  const Icon = server.transport === "stdio" ? SquareTerminal : Globe;
  return (
    <li className="ui-row plugins-row" data-server={server.id} data-status={server.status}>
      <span className="plugins-row-open">
        <PluginIcon size={28}>
          <Icon />
        </PluginIcon>
        <span className="ui-row-copy">
          <span className="ui-row-title">{server.label}</span>
          <span className="ui-row-description plugins-truncate plugins-status">{statusText(server)}</span>
        </span>
      </span>
      <div className="ui-row-control">{controls}</div>
    </li>
  );
}

interface FormState {
  key: number;
  draft: ServerDraft;
  editing: McpServer | null;
}

/** Settings › Plugins & MCPs › MCPs: configured servers with Connect, then presets that pre-fill it. */
export function McpsTab({ frame }: { frame: TabFrame }) {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.mcpServers(), [client]);
  const list = useResource<McpServerList>(load);
  const [form, setForm] = useState<FormState | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [confirm, ask] = useConfirm();

  function open(draft: ServerDraft, editing: McpServer | null = null) {
    setForm({ key: nextKey++, draft, editing });
    setFormOpen(true);
  }

  async function act(action: () => Promise<unknown>, done?: string) {
    try {
      await action();
      await list.reload();
      if (done) toast.notify(done);
    } catch (cause) {
      list.setError(errorMessage(cause));
    }
  }

  function controls(server: McpServer) {
    return (
      <>
        <Switch
          label={t("plugins.mcp.enable", { name: server.label })}
          checked={server.enabled}
          onChange={(enabled) => void act(() => client.updateMcpServer(server.id, { enabled }))}
        />
        <RowMenu name={server.label}>
          <MenuItem
            icon={<RefreshCw {...MENU_ICON} />}
            disabled={!server.enabled}
            onClick={() => void act(() => client.refreshMcpServer(server.id), t("plugins.mcp.refreshed", { name: server.label }))}
          >
            {t("plugins.mcp.refresh")}
          </MenuItem>
          <MenuItem icon={<Pencil {...MENU_ICON} />} onClick={() => open(serverDraft(server), server)}>
            {t("common.edit")}
          </MenuItem>
          <MenuItem
            danger
            icon={<Trash2 {...MENU_ICON} />}
            onClick={() =>
              ask({
                title: t("plugins.mcp.removeTitle"),
                description: t("plugins.mcp.removeDescription", { name: server.label }),
                confirmLabel: t("plugins.mcp.remove"),
                onConfirm: () => void act(() => client.deleteMcpServer(server.id), t("plugins.mcp.removed")),
              })
            }
          >
            {t("plugins.mcp.remove")}
          </MenuItem>
        </RowMenu>
      </>
    );
  }

  return (
    <>
      {frame.head(null)}
      <div className="plugins-groups" {...frame.panel}>
        <PluginGroup label={t("plugins.mcp.servers")} bare>
          <InfoBanner tone="info" title={t("plugins.mcp.bannerTitle")}>
            {t("plugins.mcp.bannerText")}
          </InfoBanner>
          <ResourceView resource={list} what={t("plugins.mcp.what")}>
            {(data) => (
              <div className="ui-card">
                <ul className="plugins-list" aria-label={t("plugins.mcp.list")}>
                  {data.servers.map((server) => (
                    <ServerRow key={server.id} server={server} controls={controls(server)} />
                  ))}
                  <li className="plugins-connect-item">
                    <button type="button" className="ui-row ui-row--single ui-row--action plugins-connect" onClick={() => open(EMPTY_DRAFT)}>
                      <Plus size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                      <span className="ui-row-title">{t("plugins.mcp.connect")}</span>
                    </button>
                  </li>
                </ul>
              </div>
            )}
          </ResourceView>
        </PluginGroup>
        <PluginGroup label={t("plugins.mcp.presets")} footnote={t("plugins.mcp.presetsNote")}>
          {MCP_PRESETS.map((preset) => {
            const Icon = PRESET_ICONS[preset.id] ?? Globe;
            const title = t(preset.title);
            return (
              <li key={preset.id} className="ui-row plugins-row" data-preset={preset.id}>
                <span className="plugins-row-open">
                  <PluginIcon size={28}>
                    <Icon />
                  </PluginIcon>
                  <span className="ui-row-copy">
                    <span className="ui-row-title">{title}</span>
                    <span className="ui-row-description plugins-truncate">{t(preset.description)}</span>
                  </span>
                </span>
                <div className="ui-row-control">
                  <IconButton size="small" className="plugins-add" label={t("plugins.mcp.addPreset", { name: title })} onClick={() => open(presetDraft(preset))}>
                    <Plus size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                </div>
              </li>
            );
          })}
        </PluginGroup>
      </div>

      <Dialog
        open={formOpen}
        onClose={() => setFormOpen(false)}
        title={form?.editing ? t("plugins.mcp.editTitle") : t("plugins.mcp.connectTitle")}
        description={form?.editing ? form.editing.label : t("plugins.mcp.connectDescription")}
      >
        {form && (
          <ServerForm
            key={form.key}
            draft={form.draft}
            editing={form.editing}
            onCancel={() => setFormOpen(false)}
            onSaved={(server) => {
              setFormOpen(false);
              toast.notify(form.editing ? t("plugins.mcp.saved") : t("plugins.mcp.added", { name: server.label }));
              void list.reload();
            }}
          />
        )}
      </Dialog>
      {confirm}
    </>
  );
}
