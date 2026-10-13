import { screen } from "@testing-library/react";
import type { UserEvent } from "@testing-library/user-event";

/** Opens a `ui/Select` and picks the option with this accessible name. */
export async function chooseOption(user: UserEvent, select: HTMLElement, option: string | RegExp) {
  await user.click(select);
  await user.click(await screen.findByRole("option", { name: option }));
}
