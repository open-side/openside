/** What the guide has done so far; the last step lists it. */
export interface Summary {
  /** Agents that have the browser skill. */
  skills: string[];
  /** The profile that was imported, as shown in the picker; `null` when nothing was. */
  imported: string | null;
  vaultReady: boolean;
  /** Name of a device paired while the guide was open. */
  phone: string | null;
}

export const EMPTY_SUMMARY: Summary = { skills: [], imported: null, vaultReady: false, phone: null };

export interface StepProps {
  update: (patch: Partial<Summary>) => void;
}
