import { BrandMark, cx } from "../ui";
import { avatarTone } from "./avatarTone";
import "./team.css";

export interface AvatarProps {
  initials: string;
  color: string;
  chief?: boolean;
  size?: 24 | 32 | 40 | 64;
  className?: string;
}

export function Avatar({ initials, color, chief = false, size = 32, className }: AvatarProps) {
  return (
    <span className={cx("avatar", `avatar--${size}`, `avatar--${avatarTone(color)}`, className)} aria-hidden="true">
      {chief ? <BrandMark size={size * 0.65} /> : initials.slice(0, 3)}
    </span>
  );
}
