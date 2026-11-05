import { ClipboardList, Scale, UserRoundCheck, type LucideIcon } from "lucide-react";
import { ICON_STROKE } from "../ui";

export interface Suggestion {
  id: string;
  icon: LucideIcon;
  title: string;
  description: string;
  /** What lands in the composer; the person finishes it, nothing is sent on pick. */
  prompt: string;
}

/** openside's own starting points for Home. Static: there is no daemon source for suggestions. */
export const HOME_SUGGESTIONS: Suggestion[] = [
  {
    id: "account",
    icon: UserRoundCheck,
    title: "Check an account",
    description: "Sign in to a site you use and report what changed since your last visit.",
    prompt: "Sign in to my account on ",
  },
  {
    id: "compare",
    icon: Scale,
    title: "Compare two offers",
    description: "Weigh two product pages on price, delivery and returns, then pick one.",
    prompt: "Compare these two offers on price, delivery and returns, and tell me which is better: ",
  },
  {
    id: "form",
    icon: ClipboardList,
    title: "Fill in a form",
    description: "Complete a form with your details and pause for your review before it is sent.",
    prompt: "Fill in this form with my details and let me review it before you submit: ",
  },
];

/** Prompt cards under the Home composer. Without suggestions the section, heading included, is absent. */
export function SuggestedTasks({ suggestions, onPick }: { suggestions: Suggestion[]; onPick: (suggestion: Suggestion) => void }) {
  if (suggestions.length === 0) return null;
  return (
    <section className="suggested-tasks" aria-labelledby="suggested-tasks-heading">
      <h2 id="suggested-tasks-heading" className="section-heading">
        Suggested tasks
      </h2>
      <ul className="suggested-list">
        {suggestions.map((suggestion) => {
          const Icon = suggestion.icon;
          return (
            <li key={suggestion.id}>
              <button type="button" className="suggested-card" data-suggestion={suggestion.id} onClick={() => onPick(suggestion)}>
                <span className="suggested-visual" aria-hidden="true">
                  <span className="suggested-tile">
                    <Icon size={24} strokeWidth={ICON_STROKE} />
                  </span>
                </span>
                <span className="suggested-body">
                  <span className="suggested-title">{suggestion.title}</span>
                  <span className="suggested-description">{suggestion.description}</span>
                  <span className="suggested-cta" aria-hidden="true">
                    Use prompt
                  </span>
                </span>
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
