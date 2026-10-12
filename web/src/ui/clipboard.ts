/**
 * Copies text; resolves false when the browser refuses. The async Clipboard API needs a secure
 * context, and a phone on a plain-HTTP tailnet address is not one, so fall back to a hidden
 * textarea with `execCommand("copy")`.
 */
export async function copyText(text: string): Promise<boolean> {
  if (window.isSecureContext && navigator.clipboard) {
    try {
      await navigator.clipboard.writeText(text);
      return true;
    } catch {
      // Permission denied: try the legacy path below.
    }
  }
  const area = document.createElement("textarea");
  area.value = text;
  area.setAttribute("readonly", "");
  area.className = "ui-offscreen";
  document.body.append(area);
  area.select();
  let copied = false;
  try {
    copied = document.execCommand("copy");
  } catch {
    copied = false;
  }
  area.remove();
  return copied;
}
