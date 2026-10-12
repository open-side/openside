/**
 * Set when the daemon refused to store that the guide was finished (this device has no admin
 * scope, or the call failed). Without it the guide would reopen on every visit to `/`.
 */
const KEY = "openside.onboarding.dismissed";

export function guideDismissed(storage: Storage = localStorage): boolean {
  return storage.getItem(KEY) !== null;
}

export function dismissGuide(storage: Storage = localStorage): void {
  storage.setItem(KEY, "1");
}
