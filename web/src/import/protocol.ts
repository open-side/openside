import type { components } from "../api/schema";

export type ImportDeliveryBatch = components["schemas"]["ImportDeliveryBatch"];
export type ImportDeliveryList = components["schemas"]["ImportDeliveryList"];
export const IMPORT_APPLY = "openside.import.apply";
export const IMPORT_STATUS = "openside.import.status";
export type ImportStatusReply = { ok: true; device: string } | { ok: false; reason: "needs_pairing" | "refused" };
export const IMPORT_PERMISSION_OPEN = "openside.import.permission.open";
export const IMPORT_PERMISSION_STATUS = "openside.import.permission.status";
export type ImportPermissionReply =
  | { ok: true; id: string; granted: boolean; denied: boolean }
  | { ok: false; reason: "needs_pairing" | "refused" | "invalid_batch" | "apply_failed" };
export type ImportReply =
  | { ok: true; id: string; bookmarks: number; history: number; cookies: number; skipped?: number }
  | { ok: false; reason: "needs_pairing" | "refused" | "invalid_batch" | "permission" | "apply_failed" };
