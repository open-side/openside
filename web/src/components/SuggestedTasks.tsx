import { ArrowUpRight } from "lucide-react";
import type { ReactNode } from "react";
import { Link } from "react-router";
import { t, type MessageKey } from "../i18n";

/** What a card does: fill the composer with a prompt (nothing is sent), or open a page. */
export type SuggestionAction = { kind: "prompt"; prompt: MessageKey } | { kind: "open"; to: string; label: MessageKey };

export interface Suggestion {
  id: string;
  glyph: ReactNode;
  title: MessageKey;
  description: MessageKey;
  action: SuggestionAction;
}

const GLYPH = { width: 48, height: 48, viewBox: "0 0 24 24", "aria-hidden": true } as const;

/** openside's own starting points for Home. Static: there is no daemon source for suggestions. */
export const HOME_SUGGESTIONS: Suggestion[] = [
  {
    id: "memory",
    glyph: (
      <svg {...GLYPH}>
        <rect x="1.5" y="1.5" width="21" height="21" rx="5.5" fill="currentColor" />
        <path d="m7.4 12.4 3.2 3.2 6-6.8" fill="none" stroke="#9aa6fb" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
      </svg>
    ),
    title: "shell.suggested.memory.title",
    description: "shell.suggested.memory.description",
    action: { kind: "prompt", prompt: "shell.suggested.memory.prompt" },
  },
  {
    id: "routines",
    glyph: (
      <svg {...GLYPH}>
        <path d="M14.3 2.6 3.1 13.6H11l-1.3 7.8 11.2-11H13Z" fill="currentColor" stroke="currentColor" strokeWidth="1.2" strokeLinejoin="round" />
      </svg>
    ),
    title: "shell.suggested.routines.title",
    description: "shell.suggested.routines.description",
    action: { kind: "prompt", prompt: "shell.suggested.routines.prompt" },
  },
  {
    id: "customize",
    glyph: (
      <svg {...GLYPH}>
        <g transform="translate(12 12) scale(1.1) translate(-12 -11.5)">
          <path d="M12 2.8c-5.2 0-9.3 3.9-9.3 8.9 0 4.7 3.8 8.5 8.6 8.5 1.4 0 2.2-.8 2.2-1.9 0-.5-.2-.9-.5-1.3-.3-.4-.5-.8-.5-1.3 0-1 .8-1.8 1.9-1.8h2.2c2.7 0 4.7-2 4.7-4.7 0-3.6-4.2-6.4-9.3-6.4Z" fill="currentColor" />
          <circle cx="7.4" cy="11.6" r="1.45" fill="#f9a35c" />
          <circle cx="9.9" cy="7.6" r="1.45" fill="#f9a35c" />
          <circle cx="14.4" cy="7.4" r="1.45" fill="#f9a35c" />
          <circle cx="17.2" cy="10.6" r="1.45" fill="#f9a35c" />
        </g>
      </svg>
    ),
    title: "shell.suggested.customize.title",
    description: "shell.suggested.customize.description",
    action: { kind: "open", to: "/settings", label: "shell.suggested.openSettings" },
  },
];

/**
 * "Suggested tasks" under Home's pill (`L 10`): gradient cards with a frosted glyph tile. A prompt
 * card hands its prompt back through `onPrompt`; an open card is a link. Without suggestions the
 * section, heading included, is absent.
 */
export function SuggestedTasks({ suggestions, onPrompt }: { suggestions: Suggestion[]; onPrompt: (id: string, prompt: MessageKey) => void }) {
  if (suggestions.length === 0) return null;
  return (
    <section className="suggested-tasks" aria-labelledby="suggested-tasks-heading">
      <h2 id="suggested-tasks-heading" className="section-heading">
        {t("shell.suggested.heading")}
      </h2>
      <ul className="suggested-list">
        {suggestions.map((suggestion) => {
          const { action } = suggestion;
          const body = (
            <>
              <span className="suggested-visual" aria-hidden="true">
                <span className="suggested-tile">{suggestion.glyph}</span>
              </span>
              <span className="suggested-body">
                <span className="suggested-title">{t(suggestion.title)}</span>
                <span className="suggested-description">{t(suggestion.description)}</span>
                <span className="suggested-cta">
                  {t(action.kind === "prompt" ? "shell.suggested.usePrompt" : action.label)}
                  <ArrowUpRight size={12} strokeWidth={1.75} aria-hidden="true" />
                </span>
              </span>
            </>
          );
          return (
            <li key={suggestion.id}>
              {action.kind === "prompt" ? (
                <button type="button" className="suggested-card" data-suggestion={suggestion.id} onClick={() => onPrompt(suggestion.id, action.prompt)}>
                  {body}
                </button>
              ) : (
                <Link to={action.to} className="suggested-card" data-suggestion={suggestion.id}>
                  {body}
                </Link>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
