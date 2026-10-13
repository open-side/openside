import { icon } from "./ui";

export interface PickerItem {
  value: string;
  label: string;
}

export interface Picker {
  value: string;
  /** Text of the current selection; the button truncates it, callers use it for a full-title tooltip. */
  readonly label: string;
  setItems(items: readonly PickerItem[], value: string): void;
}

const TYPEAHEAD_RESET_MS = 700;

/**
 * Select-only combobox (WAI-ARIA APG): the button keeps focus while a glass listbox opens under it
 * and `aria-activedescendant` follows the highlighted option. It replaces the native `<select>`,
 * whose popup the browser draws and CSS cannot style. `root` is the container the markup is built in;
 * its `data-value` mirrors the selection for tests and tooling.
 */
export function createPicker(root: HTMLElement, name: string, onChange: (value: string) => void): Picker {
  let items: readonly PickerItem[] = [];
  let value = "";
  let active = -1;
  let typed = "";
  let typedTimer: number | undefined;

  const button = document.createElement("button");
  button.type = "button";
  button.className = "picker-button";
  button.setAttribute("role", "combobox");
  button.setAttribute("aria-haspopup", "listbox");
  button.setAttribute("aria-expanded", "false");
  button.setAttribute("aria-label", name);
  const text = document.createElement("span");
  text.className = "picker-label";
  const chevron = icon("chevron-down", 16);
  chevron.classList.add("picker-chevron");
  button.append(text, chevron);

  const list = document.createElement("ul");
  list.id = `${root.id}-list`;
  list.className = "picker-list glass-float thin-scroll";
  list.setAttribute("role", "listbox");
  list.setAttribute("aria-label", name);
  list.hidden = true;
  button.setAttribute("aria-controls", list.id);
  root.append(button, list);

  const options = () => Array.from(list.children) as HTMLElement[];

  function highlight(index: number): void {
    active = index;
    options().forEach((option, i) => {
      if (i === index) option.dataset.active = "";
      else delete option.dataset.active;
    });
    const option = options()[index];
    if (option) {
      button.setAttribute("aria-activedescendant", option.id);
      option.scrollIntoView?.({ block: "nearest" });
    } else button.removeAttribute("aria-activedescendant");
  }

  function close(): void {
    typed = "";
    list.hidden = true;
    button.setAttribute("aria-expanded", "false");
    button.removeAttribute("aria-activedescendant");
  }

  function open(): void {
    list.hidden = false;
    button.setAttribute("aria-expanded", "true");
    highlight(Math.max(0, items.findIndex((item) => item.value === value)));
  }

  function set(next: string): void {
    value = next;
    const current = items.find((item) => item.value === value);
    text.textContent = current?.label ?? "";
    root.dataset.value = value;
    list.replaceChildren(
      ...items.map((item, index) => {
        const option = document.createElement("li");
        option.id = `${list.id}-${index}`;
        option.className = "picker-option";
        option.setAttribute("role", "option");
        option.setAttribute("aria-selected", String(item.value === value));
        const label = document.createElement("span");
        label.textContent = item.label;
        option.append(label);
        if (item.value === value) option.append(icon("check", 16));
        // Pressing an option must not move focus off the button, or the list would close on blur.
        option.addEventListener("mousedown", (event) => event.preventDefault());
        option.addEventListener("pointermove", () => highlight(index));
        option.addEventListener("click", () => choose(index));
        return option;
      }),
    );
  }

  function choose(index: number): void {
    const item = items[index];
    close();
    if (!item || item.value === value) return;
    set(item.value);
    onChange(item.value);
  }

  function typeahead(character: string): void {
    window.clearTimeout(typedTimer);
    typed += character.toLowerCase();
    typedTimer = window.setTimeout(() => (typed = ""), TYPEAHEAD_RESET_MS);
    const from = typed.length === 1 ? active + 1 : active;
    const hit = [...items.slice(from), ...items.slice(0, from)].find((item) => item.label.toLowerCase().startsWith(typed));
    if (hit) highlight(items.indexOf(hit));
  }

  button.addEventListener("click", () => (list.hidden ? open() : close()));
  button.addEventListener("blur", close);
  button.addEventListener("keydown", (event) => {
    const last = items.length - 1;
    if (list.hidden) {
      if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
        event.preventDefault();
        open();
      }
      return;
    }
    switch (event.key) {
      case "ArrowDown":
        highlight(Math.min(last, active + 1));
        break;
      case "ArrowUp":
        highlight(Math.max(0, active - 1));
        break;
      case "Home":
        highlight(0);
        break;
      case "End":
        highlight(last);
        break;
      case "Enter":
      case " ":
        choose(active);
        break;
      case "Escape":
        close();
        break;
      case "Tab":
        close();
        return;
      default:
        if (event.key.length === 1 && !event.ctrlKey && !event.metaKey && !event.altKey) typeahead(event.key);
        else return;
    }
    event.preventDefault();
  });

  return {
    get value() {
      return value;
    },
    set value(next: string) {
      set(next);
    },
    get label() {
      return items.find((item) => item.value === value)?.label ?? "";
    },
    setItems(next, selected) {
      items = next;
      set(selected);
    },
  };
}
