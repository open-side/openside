import { Hash, MessagesSquare, Send, type LucideIcon } from "lucide-react";
import { apiErrorMessage } from "../api/client";
import type { ChannelAccount, ChannelAccountOptions, ChannelAccountStatus, ChannelAccountTest } from "../api/types";
import { t, type MessageKey } from "../i18n";
import type { BadgeTone } from "../pages/blocks";

export type ChannelKind = "telegram" | "slack" | "discord";

export type ListKey = "allowed_chats" | "allowed_users" | "allowed_workspaces" | "allowed_channels" | "allowed_guilds";

/** Copy is stored as message keys so the list can be built at import time and translated at render. */
export interface ListField {
  key: ListKey;
  label: MessageKey;
  placeholder: string;
  hint: MessageKey;
}

export interface Service {
  kind: ChannelKind;
  name: string;
  Icon: LucideIcon;
  /** The account dialog's subtitle: how the service reaches openside. */
  blurb: MessageKey;
  /** The platform row's second line on Settings › Channels while it has no accounts. */
  tagline: MessageKey;
  lists: ListField[];
  /** Slack verifies inbound callbacks with a signing secret next to the bot token. */
  signingSecret: boolean;
  /** Slack and Discord call openside back on a signed URL; Telegram is polled. `field` is the service's own (untranslated) field name. */
  callback: { field: string; steps: MessageKey } | null;
}

export const SERVICES: Service[] = [
  {
    kind: "telegram",
    name: "Telegram",
    Icon: Send,
    blurb: "models.channels.telegram.blurb",
    tagline: "models.channels.telegram.tagline",
    lists: [
      { key: "allowed_chats", label: "models.channels.telegram.chats", placeholder: "123456789, -1001234567890", hint: "models.channels.telegram.chatsHint" },
      { key: "allowed_users", label: "models.channels.allowedPeople", placeholder: "987654321", hint: "models.channels.telegram.usersHint" },
    ],
    signingSecret: false,
    callback: null,
  },
  {
    kind: "slack",
    name: "Slack",
    Icon: Hash,
    blurb: "models.channels.slack.blurb",
    tagline: "models.channels.slack.tagline",
    lists: [
      { key: "allowed_workspaces", label: "models.channels.slack.workspaces", placeholder: "T0123ABCD", hint: "models.channels.slack.workspacesHint" },
      { key: "allowed_channels", label: "models.channels.allowedChannels", placeholder: "C0123ABCD, D0456EFGH", hint: "models.channels.slack.channelsHint" },
      { key: "allowed_users", label: "models.channels.allowedPeople", placeholder: "U0123ABCD", hint: "models.channels.slack.usersHint" },
    ],
    signingSecret: true,
    callback: { field: "Request URL", steps: "models.channels.slack.steps" },
  },
  {
    kind: "discord",
    name: "Discord",
    Icon: MessagesSquare,
    blurb: "models.channels.discord.blurb",
    tagline: "models.channels.discord.tagline",
    lists: [
      { key: "allowed_guilds", label: "models.channels.discord.guilds", placeholder: "112233445566778899", hint: "models.channels.discord.guildsHint" },
      { key: "allowed_channels", label: "models.channels.allowedChannels", placeholder: "998877665544332211", hint: "models.channels.discord.channelsHint" },
      { key: "allowed_users", label: "models.channels.allowedPeople", placeholder: "123123123123123123", hint: "models.channels.discord.usersHint" },
    ],
    signingSecret: false,
    callback: { field: "Interactions Endpoint URL", steps: "models.channels.discord.steps" },
  },
];

export const serviceOf = (kind: string): Service => SERVICES.find((service) => service.kind === kind) ?? SERVICES[0]!;

export const ID_PATTERN = /^[a-z0-9_-]{1,40}$/;

export function slugify(label: string): string {
  return label
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
}

export const listText = (list: readonly string[] | undefined) => (list ?? []).join(", ");

export function parseList(text: string): string[] {
  return [...new Set(text.split(/[\s,]+/).filter(Boolean))];
}

export const hasAllowList = (options: ChannelAccountOptions) =>
  [options.allowed_chats, options.allowed_users, options.allowed_workspaces, options.allowed_channels, options.allowed_guilds].some((list) => list.length > 0);

export function statusView(status: ChannelAccountStatus): { text: string; tone: BadgeTone } {
  switch (status) {
    case "connected":
      return { text: t("models.channels.status.connected"), tone: "success" };
    case "disabled":
      return { text: t("common.off"), tone: "neutral" };
    case "expired":
      return { text: t("models.channels.status.signInAgain"), tone: "danger" };
    case "unavailable":
      return { text: t("models.channels.status.unavailable"), tone: "attention" };
    case "vault_locked":
      return { text: t("models.channels.status.vaultLocked"), tone: "attention" };
    case "restart_required":
      return { text: t("models.channels.status.restartNeeded"), tone: "attention" };
  }
}

/** One sentence for a connection test; codes are the daemon's allow-listed ones, never upstream text. */
export function testSummary(result: ChannelAccountTest): string {
  const fallback = result.ok ? t("models.channels.test.passed") : t("models.channels.test.failed");
  const message = apiErrorMessage(result.code === "ok" ? "channel_ok" : result.code) ?? fallback;
  return result.restart_required ? t("models.channels.test.restart", { message }) : message;
}

/** A callback address on this computer cannot be reached by Slack or Discord. */
export function isLoopbackUrl(url: string): boolean {
  try {
    const host = new URL(url).hostname.replace(/^\[|\]$/g, "");
    return host === "localhost" || host === "::1" || host.startsWith("127.") || host.endsWith(".localhost");
  } catch {
    return false;
  }
}

export const isLegacy = (account: ChannelAccount) => account.credential_source === "legacy";
