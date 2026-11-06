/** The guide's steps in order; `?step=` carries one of these. */
export type StepId = "welcome" | "import" | "passwords" | "subscription" | "done";

/** What the guide has done so far. */
export interface Summary {
  /** What the browser import brought over; `null` until something was imported. */
  imported: string | null;
  /** Passwords added to the vault in the passwords step; `null` until an import finished. */
  passwords: number | null;
  /** Providers that have a usable model; `null` until one was connected in the guide. */
  models: string | null;
}

export const EMPTY_SUMMARY: Summary = { imported: null, passwords: null, models: null };

/** ImportStep's contract with whoever hosts it (the guide, or Settings › General's dialog). */
export interface StepProps {
  update: (patch: Partial<Summary>) => void;
}

/** What the page hands every step: moving between steps, ending the guide, and the shared summary. */
export interface StepNav extends StepProps {
  next: () => void;
  back: () => void;
  /** Ends the guide, records the outcome while it is still pending, then opens `to` (default: home). */
  leave: (outcome: "completed" | "skipped", to?: string) => Promise<void>;
  /** `leave` is running; steps disable their way out meanwhile. */
  leaving: boolean;
  summary: Summary;
}
