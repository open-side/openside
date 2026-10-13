/** What the guide has done so far; the last step lists it. */
export interface Summary {
  /** Providers that have a usable model, as shown in the guide; `null` until one does. */
  models: string | null;
  /** Agents that have the browser skill. */
  skills: string[];
  /** What the browser import brought over, as shown in the recap; `null` when nothing was imported. */
  imported: string | null;
  /** Name of a device paired while the guide was open. */
  phone: string | null;
  /** The assistant's name once confirmed or changed in the guide. */
  assistant: string | null;
}

export const EMPTY_SUMMARY: Summary = { models: null, skills: [], imported: null, phone: null, assistant: null };

export interface StepProps {
  update: (patch: Partial<Summary>) => void;
}
