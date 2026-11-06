import { Menu as BaseMenu } from "@base-ui/react/menu";
import { Copy, GitBranch, TextQuote, TextSelect } from "lucide-react";
import { useEffect, useRef, useState, type HTMLAttributes, type KeyboardEvent, type MouseEvent, type PointerEvent, type ReactNode, type SyntheticEvent } from "react";
import { formatDate, t } from "../i18n";
import { haptic } from "../pwa/haptics";
import { ICON_STROKE, IconButton, MenuItem, copyText, useToast } from "../ui";

const LONG_PRESS_MS = 500;
/** A finger that travels further than this is scrolling or swiping, not pressing. */
const PRESS_SLOP_PX = 8;

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/** `text` as a Markdown quote followed by a blank line, ready to be written under. */
export function quoteText(text: string): string {
  const lines = text.trim().split("\n").map((line) => (line.trim() ? `> ${line}` : ">"));
  return `${lines.join("\n")}\n\n`;
}

/**
 * Whether a press belongs to the browser: links, media and controls keep their own menus and
 * gestures, and so does text the person has already selected inside the message.
 */
function nativeTarget(event: SyntheticEvent<HTMLElement>): boolean {
  if (event.target instanceof Element && event.target.closest("a, img, video, input, textarea, button")) return true;
  const selection = window.getSelection();
  return Boolean(selection && !selection.isCollapsed && event.currentTarget.contains(selection.anchorNode));
}

/** Touch screens make bubbles unselectable so a long press reaches the menu; only they need Select text. */
function coarsePointer(): boolean {
  return window.matchMedia?.("(pointer: coarse)").matches ?? false;
}

/** Copies a message and says so; shared by the menu and the hover row under a reply. */
function useCopy(text: string): () => void {
  const toast = useToast();
  return () =>
    void copyText(text).then((copied) => (copied ? toast.notify(t("common.copied")) : toast.notify(t("thread.message.copyFailed"), "error")));
}

export interface MessageMenuProps extends HTMLAttributes<HTMLElement> {
  /** The element the message is drawn as: a timeline row, or a queued bubble's paragraph. */
  element?: "li" | "p";
  /** What Copy and Quote take: the message's own words, Markdown included. */
  text: string;
  /** Puts the quoted message into the composer; absent where nothing can be written. */
  onQuote?: ((text: string) => void) | undefined;
  /** Items for this kind of message only, after Copy and Quote (a queued message's Edit). */
  extra?: ReactNode;
  children: ReactNode;
}

/**
 * A message that opens its actions on a right click, a long press, or Shift+F10 / the Menu key
 * once focused: Copy, Quote, and whatever this kind of message adds. Links, images and selected
 * text keep the browser's own menu.
 */
export function MessageMenu({ element: Tag = "li", text, onQuote, extra, children, ...props }: MessageMenuProps) {
  const [open, setOpen] = useState(false);
  const [point, setPoint] = useState<{ x: number; y: number } | null>(null);
  // Select text: the bubble takes native selection (and its long press) until a tap lands outside it.
  const [selectable, setSelectable] = useState(false);
  const [offerSelect, setOfferSelect] = useState(false);
  const node = useRef<HTMLElement | null>(null);
  const press = useRef<{ x: number; y: number; timer: number } | null>(null);
  // The click that ends a long press must not also follow a link under the finger.
  const swallowClick = useRef(false);
  const copy = useCopy(text);

  // Scrolling the thread while a finger rests on a message is not a press; the listener lives
  // only as long as the press, so a long thread does not carry one per message.
  const cancel = useRef(() => {
    if (!press.current) return;
    window.clearTimeout(press.current.timer);
    press.current = null;
    window.removeEventListener("scroll", cancel.current, { capture: true });
  });
  useEffect(() => cancel.current, []);

  useEffect(() => {
    if (!selectable) return;
    // Start from the whole message, once it is selectable and the menu has let go of focus, so
    // the selection handles appear; the person narrows it from there.
    const frame = window.requestAnimationFrame(() => {
      if (node.current) window.getSelection()?.selectAllChildren(node.current);
    });
    const leave = (event: Event) => {
      if (event.target instanceof Node && node.current?.contains(event.target)) return;
      setSelectable(false);
    };
    document.addEventListener("pointerdown", leave, { capture: true });
    return () => {
      window.cancelAnimationFrame(frame);
      document.removeEventListener("pointerdown", leave, { capture: true });
    };
  }, [selectable]);

  const show = (x: number, y: number) => {
    setOfferSelect(coarsePointer());
    setPoint({ x, y });
    setOpen(true);
  };

  const selectText = () => setSelectable(true);

  const enabled = text.trim().length > 0;
  const handlers = enabled
    ? {
        onContextMenu: (event: MouseEvent<HTMLElement>) => {
          if (selectable || nativeTarget(event)) return;
          event.preventDefault();
          cancel.current();
          show(event.clientX, event.clientY);
        },
        onPointerDown: (event: PointerEvent<HTMLElement>) => {
          swallowClick.current = false;
          if (event.pointerType !== "touch" || !event.isPrimary || selectable || nativeTarget(event)) return;
          cancel.current();
          const { clientX: x, clientY: y } = event;
          const timer = window.setTimeout(() => {
            cancel.current();
            swallowClick.current = true;
            haptic("approve");
            show(x, y);
          }, LONG_PRESS_MS);
          press.current = { x, y, timer };
          window.addEventListener("scroll", cancel.current, { capture: true, passive: true });
        },
        onPointerMove: (event: PointerEvent<HTMLElement>) => {
          const start = press.current;
          if (start && Math.hypot(event.clientX - start.x, event.clientY - start.y) > PRESS_SLOP_PX) cancel.current();
        },
        onPointerUp: cancel.current,
        onPointerCancel: cancel.current,
        onClickCapture: (event: MouseEvent<HTMLElement>) => {
          if (!swallowClick.current) return;
          swallowClick.current = false;
          event.preventDefault();
          event.stopPropagation();
        },
        // Keyboard users reach the same menu from the focused message, opened at its corner.
        tabIndex: 0,
        "aria-haspopup": "menu" as const,
        "aria-keyshortcuts": "Shift+F10",
        onKeyDown: (event: KeyboardEvent<HTMLElement>) => {
          props.onKeyDown?.(event);
          if (event.target !== event.currentTarget || selectable) return;
          if (!(event.key === "ContextMenu" || (event.key === "F10" && event.shiftKey))) return;
          event.preventDefault();
          const box = event.currentTarget.getBoundingClientRect();
          show(box.left, box.top + Math.min(box.height, 24));
        },
      }
    : {};

  return (
    <>
      <Tag
        {...props}
        {...handlers}
        ref={(element: HTMLElement | null) => {
          node.current = element;
        }}
        data-selectable={selectable ? "" : undefined}
      >
        {children}
      </Tag>
      {/* Mounted on first use: a long thread would otherwise hold one menu per message. */}
      {point ? (
        <BaseMenu.Root open={open} onOpenChange={setOpen}>
          <BaseMenu.Portal>
            <BaseMenu.Positioner
              className="ui-menu-positioner"
              anchor={{ getBoundingClientRect: () => DOMRect.fromRect({ x: point.x, y: point.y, width: 0, height: 0 }) }}
              side="bottom"
              align="start"
              sideOffset={4}
              collisionPadding={8}
            >
              <BaseMenu.Popup className="ui-menu surface-overlay" aria-label={t("thread.message.actions")}>
                <MenuItem icon={<Copy {...icon} />} onClick={copy}>
                  {t("common.copy")}
                </MenuItem>
                {onQuote ? (
                  <MenuItem icon={<TextQuote {...icon} />} onClick={() => onQuote(text)}>
                    {t("thread.message.quote")}
                  </MenuItem>
                ) : null}
                {offerSelect ? (
                  <MenuItem icon={<TextSelect {...icon} />} onClick={selectText}>
                    {t("thread.message.selectText")}
                  </MenuItem>
                ) : null}
                {extra}
              </BaseMenu.Popup>
            </BaseMenu.Positioner>
          </BaseMenu.Portal>
        </BaseMenu.Root>
      ) : null}
    </>
  );
}

/**
 * The row under a reply: Copy, Branch (a new chat holding the conversation up to this reply) and
 * the reply's time. Always shown under the newest reply, on hover under older ones.
 */
export function ReplyFooter({ text, at, onFork }: { text: string; at?: string | undefined; onFork?: () => Promise<void> }) {
  const copy = useCopy(text);
  const [forking, setForking] = useState(false);
  const time = at && !Number.isNaN(Date.parse(at)) ? at : null;
  return (
    <div className="message-footer">
      {text ? (
        <IconButton label={t("thread.message.copy")} size="small" onClick={copy}>
          <Copy size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      ) : null}
      {onFork ? (
        <IconButton
          label={t("thread.message.branch")}
          size="small"
          disabled={forking}
          onClick={() => {
            setForking(true);
            void onFork().finally(() => setForking(false));
          }}
        >
          <GitBranch size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      ) : null}
      {time ? <time dateTime={time}>{formatDate(new Date(time), { hour: "numeric", minute: "2-digit" })}</time> : null}
    </div>
  );
}
