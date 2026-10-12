/**
 * The openside mark drawn in `currentColor`, so it follows the theme text colour. The app icon
 * (`public/icons/icon.svg`) is the same mark, scaled onto a fixed dark tile; keep the two in sync.
 */
export function BrandMark({ size }: { size: number }) {
  return (
    <svg viewBox="-96 -96 192 192" width={size} height={size} fill="currentColor" aria-hidden="true">
      <path d="M0 96A96 96 0 1 1 90.18 -32.92L61.28 -18.47A64 64 0 1 0 0 64ZM46 16L94 -8L94 56L46 80Z" />
    </svg>
  );
}
