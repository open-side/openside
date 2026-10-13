import { cx } from "../ui";
import { avatarTone } from "./avatarTone";
import "./team.css";

export interface AvatarProps {
  initials: string;
  color: string;
  size?: 24 | 32 | 40 | 64;
  /** Three dots over the corner while the agent is working. */
  working?: boolean;
  className?: string;
}

export function Avatar({ initials, color, size = 32, working = false, className }: AvatarProps) {
  return (
    <span className={cx("avatar", `avatar--${size}`, `avatar--${avatarTone(color)}`, className)} aria-hidden="true">
      {initials.slice(0, 3)}
      {working ? (
        <span className="avatar-working">
          <span className="typing-dot" />
          <span className="typing-dot" />
          <span className="typing-dot" />
        </span>
      ) : null}
    </span>
  );
}
