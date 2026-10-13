import { Hash, MessagesSquare, Send, type LucideIcon } from "lucide-react";
import { MESSAGES } from "../api/client";
import type { ChannelAccount, ChannelAccountOptions, ChannelAccountStatus, ChannelAccountTest } from "../api/types";
import type { BadgeTone } from "../pages/blocks";

export type ChannelKind = "telegram" | "slack" | "discord";

export type ListKey = "allowed_chats" | "allowed_users" | "allowed_workspaces" | "allowed_channels" | "allowed_guilds";

export interface ListField {
  key: ListKey;
  label: string;
  placeholder: string;
  hint: string;
}

export interface Service {
  kind: ChannelKind;
  name: string;
  Icon: LucideIcon;
  blurb: string;
  lists: ListField[];
  /** Slack verifies inbound callbacks with a signing secret next to the bot token. */
  signingSecret: boolean;
  /** Slack and Discord call openside back on a signed URL; Telegram is polled. */
  callback: { field: string; steps: string } | null;
}

export const SERVICES: Service[] = [
  {
    kind: "telegram",
    name: "Telegram",
    Icon: Send,
    blurb: "Each bot is polled by openside, so no public address is needed.",
    lists: [
      { key: "allowed_chats", label: "Allowed chats", placeholder: "123456789, -1001234567890", hint: "Chat ids this bot may answer." },
      { key: "allowed_users", label: "Allowed people", placeholder: "987654321", hint: "Telegram user ids; leave empty to allow everyone in an allowed chat." },
    ],
    signingSecret: false,
    callback: null,
  },
  {
    kind: "slack",
    name: "Slack",
    Icon: Hash,
    blurb: "One account per Slack workspace app. Slack calls openside back on a signed address.",
    lists: [
      { key: "allowed_workspaces", label: "Allowed workspaces", placeholder: "T0123ABCD", hint: "Workspace ids, like T0123ABCD." },
      { key: "allowed_channels", label: "Allowed channels", placeholder: "C0123ABCD, D0456EFGH", hint: "Channel or conversation ids." },
      { key: "allowed_users", label: "Allowed people", placeholder: "U0123ABCD", hint: "Slack member ids; leave empty to allow everyone in an allowed channel." },
    ],
    signingSecret: true,
    callback: { field: "Request URL", steps: "In your Slack app, open Event Subscriptions, turn them on and paste this address as the Request URL." },
  },
  {
    kind: "discord",
    name: "Discord",
    Icon: MessagesSquare,
    blurb: "One account per Discord application. Discord calls openside back on a signed address.",
    lists: [
      { key: "allowed_guilds", label: "Allowed servers", placeholder: "112233445566778899", hint: "Server (guild) ids." },
      { key: "allowed_channels", label: "Allowed channels", placeholder: "998877665544332211", hint: "Channel ids." },
      { key: "allowed_users", label: "Allowed people", placeholder: "123123123123123123", hint: "Discord user ids; leave empty to allow everyone in an allowed channel." },
    ],
    signingSecret: false,
    callback: { field: "Interactions Endpoint URL", steps: "In the Discord Developer Portal, open your application's General Information and paste this address as the Interactions Endpoint URL." },
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
      return { text: "Connected", tone: "success" };
    case "disabled":
      return { text: "Off", tone: "neutral" };
    case "expired":
      return { text: "Sign in again", tone: "danger" };
    case "unavailable":
      return { text: "Unavailable", tone: "attention" };
    case "vault_locked":
      return { text: "Vault locked", tone: "attention" };
    case "restart_required":
      return { text: "Restart needed", tone: "attention" };
  }
}

/** One sentence for a connection test; codes are the daemon's allow-listed ones, never upstream text. */
export function testSummary(result: ChannelAccountTest): string {
  const fallback = result.ok ? "The service accepted this account's credentials." : "The check did not pass.";
  const message = MESSAGES[result.code === "ok" ? "channel_ok" : result.code] ?? fallback;
  return result.restart_required ? `${message} Restart openside to start this account.` : message;
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
