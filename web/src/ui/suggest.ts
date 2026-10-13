/**
 * Imperative suggestion combobox for a text input (WAI-ARIA APG "editable combobox with list
 * autocomplete"). Shared by the web app and the extension side panel, whose login cards are built
 * without React (cards/fields.ts). It replaces `<datalist>`, whose popup the browser draws and
 * CSS cannot style. The input keeps focus; the listbox is a sibling inside the input's
 * `position: relative` wrapper (see `.oc-suggest-list` in cards/cards.css).
 */
export function attachSuggestions(input: HTMLInputElement | HTMLTextAreaElement, suggestions: readonly string[]): void {
  const wrap = input.parentElement;
  if (!wrap || suggestions.length === 0) return;
  const listId = `${input.id || "suggest"}-list`;
  const list = document.createElement("ul");
  list.id = listId;
  list.className = "oc-suggest-list";
  list.setAttribute("role", "listbox");
  list.setAttribute("aria-label", input.getAttribute("placeholder") ?? "Suggestions");
  list.hidden = true;
  wrap.append(list);
  input.setAttribute("role", "combobox");
  input.setAttribute("aria-autocomplete", "list");
  input.setAttribute("aria-expanded", "false");
  input.setAttribute("aria-controls", listId);

  let shown: string[] = [];
  let active = -1;

  function highlight(index: number): void {
    active = index;
    [...list.children].forEach((option, i) => {
      option.toggleAttribute("data-active", i === index);
    });
    const option = list.children[index];
    if (option) {
      input.setAttribute("aria-activedescendant", option.id);
      option.scrollIntoView?.({ block: "nearest" });
    } else input.removeAttribute("aria-activedescendant");
  }

  function close(): void {
    list.hidden = true;
    input.setAttribute("aria-expanded", "false");
    input.removeAttribute("aria-activedescendant");
    active = -1;
  }

  function pick(index: number): void {
    const value = shown[index];
    close();
    if (value === undefined) return;
    input.value = value;
    input.dispatchEvent(new Event("input", { bubbles: true }));
  }

  function refresh(): void {
    const typed = input.value.trim().toLowerCase();
    shown = suggestions.filter((value) => value.toLowerCase() !== typed && value.toLowerCase().includes(typed));
    if (shown.length === 0) {
      close();
      return;
    }
    list.replaceChildren(
      ...shown.map((value, index) => {
        const option = document.createElement("li");
        option.id = `${listId}-${index}`;
        option.className = "oc-suggest-option";
        option.setAttribute("role", "option");
        option.setAttribute("aria-selected", "false");
        option.textContent = value;
        // Pressing an option must not blur the input, which would close the list before the click lands.
        option.addEventListener("mousedown", (event) => event.preventDefault());
        option.addEventListener("pointermove", () => highlight(index));
        option.addEventListener("click", () => pick(index));
        return option;
      }),
    );
    list.hidden = false;
    input.setAttribute("aria-expanded", "true");
    highlight(-1);
  }

  input.addEventListener("focus", refresh);
  input.addEventListener("input", refresh);
  input.addEventListener("blur", close);
  // Typed as the shared base so the union of input and text area still yields KeyboardEvent.
  const control: HTMLElement = input;
  control.addEventListener("keydown", (event) => {
    // A multi-line field keeps ArrowDown for moving the caret until the list is already open.
    if (event.key === "ArrowDown" && list.hidden && input instanceof HTMLInputElement) {
      refresh();
      event.preventDefault();
      return;
    }
    if (list.hidden) return;
    switch (event.key) {
      case "ArrowDown":
        highlight((active + 1) % shown.length);
        break;
      case "ArrowUp":
        highlight(active <= 0 ? shown.length - 1 : active - 1);
        break;
      case "Enter":
        // Only an explicitly highlighted suggestion is taken; otherwise Enter submits the form as usual.
        if (active < 0) return;
        pick(active);
        break;
      case "Escape":
        close();
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  });
}
