import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { en as extEn } from "../../../../extension/src/i18n/en";
import { ko as extKo } from "../../../../extension/src/i18n/ko";
import { EXTENSION_ID } from "../../desktopNotifyProtocol";
import { en, ko } from "../../i18n/catalog";
import { DEFAULT_LASSO, REGION_BUILTINS, TEXT_BUILTINS, type LassoReply, type LassoRequest, type LassoSettings } from "../../lassoProtocol";
import { ToastProvider } from "../../ui";
import { LassoSettingsPage } from "./Lasso";

interface Extension {
  requests: LassoRequest[];
  settings: LassoSettings;
  access: boolean;
}

/** Installs a `chrome.runtime` that answers like the openside extension's Lasso handler. */
function fakeExtension(settings: LassoSettings = DEFAULT_LASSO, access = true, command = "Alt+Shift+L"): Extension {
  const extension: Extension = { requests: [], settings: structuredClone(settings), access };
  Reflect.set(globalThis, "chrome", {
    runtime: {
      lastError: undefined,
      sendMessage(id: string, request: LassoRequest, callback: (reply: LassoReply) => void) {
        expect(id).toBe(EXTENSION_ID);
        extension.requests.push(request);
        if (request.type === "openside.lasso.set") extension.settings = request.settings;
        queueMicrotask(() => callback({ ok: true, settings: structuredClone(extension.settings), access: extension.access, command }));
      },
    },
  });
  return extension;
}

function show() {
  render(
    <ToastProvider>
      <LassoSettingsPage />
    </ToastProvider>,
  );
}

function sets(extension: Extension): LassoSettings[] {
  return extension.requests.flatMap((request) => (request.type === "openside.lasso.set" ? [request.settings] : []));
}

afterEach(() => {
  Reflect.deleteProperty(globalThis, "chrome");
});

describe("Settings › Lasso", () => {
  it("shows the extension's settings and switches text selection off", async () => {
    const user = userEvent.setup();
    const extension = fakeExtension();
    show();
    expect(screen.getByRole("heading", { name: "Select and take action" })).toBeInTheDocument();
    const toggle = screen.getByRole("switch", { name: "Enable on text selection" });
    await waitFor(() => expect(toggle).toBeEnabled());
    expect(toggle).toBeChecked();
    expect(screen.getByText("Summarize")).toBeInTheDocument();
    expect(screen.getByText("Search image")).toBeInTheDocument();

    await user.click(toggle);
    await waitFor(() => expect(sets(extension)).toHaveLength(1));
    expect(sets(extension)[0]!.text.enabled).toBe(false);
    expect(sets(extension)[0]!.region).toEqual(DEFAULT_LASSO.region);
    expect(toggle).not.toBeChecked();
  });

  it("adds a shortcut to the text selection card", async () => {
    const user = userEvent.setup();
    const extension = fakeExtension();
    show();
    const [addText] = await screen.findAllByRole("button", { name: "Add shortcut" });
    await waitFor(() => expect(addText).toBeEnabled());
    await user.click(addText!);
    const dialog = await screen.findByRole("dialog", { name: "Add shortcut" });
    await user.type(within(dialog).getByLabelText("Name"), "Explain");
    await user.type(within(dialog).getByLabelText("Instruction"), "Explain this simply.");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(sets(extension)).toHaveLength(1));
    const added = sets(extension)[0]!.text.shortcuts[2]!;
    expect(added).toMatchObject({ label: "Explain", prompt: "Explain this simply." });
    expect(added.id).toMatch(/^custom-[a-z0-9]{1,32}$/);
    expect(await screen.findByText("Explain")).toBeInTheDocument();
  });

  it("edits a built-in shortcut, starting from its own words", async () => {
    const user = userEvent.setup();
    const extension = fakeExtension();
    show();
    const options = screen.getByRole("button", { name: "Options for Summarize" });
    await waitFor(() => expect(options).toBeEnabled());
    await user.click(options);
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const dialog = await screen.findByRole("dialog", { name: "Edit shortcut" });
    expect(within(dialog).getByLabelText("Instruction")).toHaveValue("Summarize this.");
    const name = within(dialog).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Short summary");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await waitFor(() => expect(sets(extension)).toHaveLength(1));
    expect(sets(extension)[0]!.text.shortcuts[0]).toEqual({ id: "summarize", label: "Short summary", prompt: "Summarize this." });
  });

  it("removes a lasso shortcut", async () => {
    const user = userEvent.setup();
    const extension = fakeExtension();
    show();
    const options = screen.getByRole("button", { name: "Options for Copy code" });
    await waitFor(() => expect(options).toBeEnabled());
    await user.click(options);
    await user.click(await screen.findByRole("menuitem", { name: "Remove" }));

    await waitFor(() => expect(sets(extension)).toHaveLength(1));
    expect(sets(extension)[0]!.region.shortcuts).toEqual([{ id: "search-image" }]);
    await waitFor(() => expect(screen.queryByText("Copy code")).not.toBeInTheDocument());
  });

  it("changes the hotkey", async () => {
    const user = userEvent.setup();
    const extension = fakeExtension();
    show();
    const hotkey = screen.getByRole("combobox", { name: "Hotkey" });
    await waitFor(() => expect(hotkey).not.toHaveAttribute("data-disabled"));
    await user.click(hotkey);
    await user.click(await screen.findByRole("option", { name: /Shift \+ Shift|⇧ \+ ⇧/ }));

    await waitFor(() => expect(sets(extension)).toHaveLength(1));
    expect(sets(extension)[0]!.region.hotkey).toBe("shift");
    expect(screen.getByText(/Press (Shift|⇧) twice and draw a circle\./)).toBeInTheDocument();
  });

  it("shows the defaults, disabled, when no extension answers", async () => {
    show();
    expect(await screen.findByText("Open Settings in the openside browser to change Lasso.")).toBeInTheDocument();
    expect(screen.getByRole("switch", { name: "Enable on text selection" })).toBeDisabled();
    expect(screen.getByRole("switch", { name: "Enable on lasso selection" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Options for Translate" })).toBeDisabled();
    for (const add of screen.getAllByRole("button", { name: "Add shortcut" })) expect(add).toBeDisabled();
  });

  it("asks the extension for site access while Lasso lacks it", async () => {
    const user = userEvent.setup();
    const extension = fakeExtension(DEFAULT_LASSO, false);
    show();
    expect(await screen.findByText(/Lasso needs access to every site/)).toBeInTheDocument();
    expect(screen.getByText(/Alt\+Shift\+L and right-click › Capture work on any page\./)).toBeInTheDocument();
    extension.access = true;
    await user.click(screen.getByRole("button", { name: "Allow" }));

    expect(extension.requests.at(-1)).toEqual({ type: "openside.lasso.access" });
    await waitFor(() => expect(screen.queryByText(/Lasso needs access to every site/)).not.toBeInTheDocument());
  });

  it("names built-ins with the same words as the extension", () => {
    for (const id of [...TEXT_BUILTINS, ...REGION_BUILTINS]) {
      for (const part of ["label", "prompt"] as const) {
        expect(en[`settings.lasso.builtin.${id}.${part}`]).toBe(extEn[`ext.lasso.builtin.${id}.${part}`]);
        expect(ko[`settings.lasso.builtin.${id}.${part}`]).toBe(extKo[`ext.lasso.builtin.${id}.${part}`]);
      }
    }
  });
});
