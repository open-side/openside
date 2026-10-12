import { CircleAlert, CircleCheck, X } from "lucide-react";
import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import { ICON_STROKE } from "./Button";

export type ToastTone = "success" | "error";

export interface Toaster {
  notify: (message: string, tone?: ToastTone) => void;
}

interface Toast {
  id: number;
  message: string;
  tone: ToastTone;
  /** Set while the exit transition plays; the card is removed afterwards. */
  leaving?: boolean;
}

const STACK_LIMIT = 3;
const DURATION: Record<ToastTone, number> = { success: 3000, error: 6000 };
// Matches --normal in styles.css.
const LEAVE_MS = 220;

// Components rendered outside the shell (unit tests, sign-in) still get a working no-op.
const ToastContext = createContext<Toaster>({ notify: () => undefined });

export const useToast = () => useContext(ToastContext);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(0);
  const timers = useRef(new Map<number, number>());

  const dismiss = useCallback((id: number) => {
    clearTimeout(timers.current.get(id));
    setToasts((current) => current.map((toast) => (toast.id === id ? { ...toast, leaving: true } : toast)));
    timers.current.set(
      id,
      window.setTimeout(() => {
        timers.current.delete(id);
        setToasts((current) => current.filter((toast) => toast.id !== id));
      }, LEAVE_MS),
    );
  }, []);

  const notify = useCallback(
    (message: string, tone: ToastTone = "success") => {
      const id = ++nextId.current;
      setToasts((current) => [...current, { id, message, tone }].slice(-STACK_LIMIT));
      timers.current.set(
        id,
        window.setTimeout(() => dismiss(id), DURATION[tone]),
      );
    },
    [dismiss],
  );

  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach((timer) => clearTimeout(timer));
  }, []);

  const value = useMemo(() => ({ notify }), [notify]);

  return (
    <ToastContext.Provider value={value}>
      {children}
      {createPortal(
        <div className="ui-toasts">
          {toasts.map((toast, index) => {
            const depth = toasts.length - 1 - index;
            return (
              <div
                key={toast.id}
                className={`ui-toast ui-toast--${toast.tone}`}
                data-depth={depth}
                data-leaving={toast.leaving || undefined}
                role={toast.tone === "error" ? "alert" : "status"}
                style={{ zIndex: STACK_LIMIT - depth, transform: `translateY(${depth * 8}px) scale(${1 - depth * 0.04})` }}
              >
                {toast.tone === "error" ? (
                  <CircleAlert size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                ) : (
                  <CircleCheck size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
                )}
                <span className="ui-toast-message">{toast.message}</span>
                <button type="button" className="ui-toast-close" aria-label="Dismiss notification" onClick={() => dismiss(toast.id)}>
                  <X size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </button>
              </div>
            );
          })}
        </div>,
        document.body,
      )}
    </ToastContext.Provider>
  );
}
