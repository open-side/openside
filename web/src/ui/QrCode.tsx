import { useMemo } from "react";
import { encode } from "uqr";

/** QR spec quiet zone; scanners reject codes with less white margin than this. */
const QUIET_ZONE = 4;

/**
 * Renders `text` as a QR code in the browser (no QR service ever sees the pairing code).
 * Always black on white regardless of theme: dark-mode scanners fail on inverted codes.
 */
export function QrCode({ text, label, size = 224 }: { text: string; label: string; size?: number }) {
  const { path, extent } = useMemo(() => {
    const { data, size: modules } = encode(text, { ecc: "M", border: 0 });
    let d = "";
    for (let y = 0; y < modules; y++) {
      const row = data[y] ?? [];
      // One horizontal run per path segment keeps the markup small for version-10+ codes.
      for (let x = 0; x < modules; x++) {
        if (!row[x]) continue;
        let end = x;
        while (end + 1 < modules && row[end + 1]) end++;
        d += `M${x + QUIET_ZONE} ${y + QUIET_ZONE}h${end - x + 1}v1h-${end - x + 1}z`;
        x = end;
      }
    }
    return { path: d, extent: modules + QUIET_ZONE * 2 };
  }, [text]);

  return (
    <svg
      className="qr-code"
      role="img"
      aria-label={label}
      width={size}
      height={size}
      viewBox={`0 0 ${extent} ${extent}`}
      shapeRendering="crispEdges"
    >
      <rect width={extent} height={extent} fill="#fff" />
      <path d={path} fill="#000" />
    </svg>
  );
}
