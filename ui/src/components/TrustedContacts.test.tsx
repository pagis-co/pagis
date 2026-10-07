// The Trusted contacts section (ADR-0021, ADR-0019): the keypad
// code first, then phone numbers and email senders as framed tables,
// and the tier explainer as the closing hint.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import { describe, expect, it, onTestFinished, vi } from "vitest";

import type { ApiClient } from "../api/client";
import { formatClock } from "../timeline";
import { TrustedContacts } from "./TrustedContacts";

const page = {
  items: [
    {
      id: "row-1",
      agent_id: null,
      subject: "number",
      value: "+14155550123",
      tier: "owner",
      label: "Home",
      created_at: 1,
    },
    {
      id: "row-2",
      agent_id: null,
      subject: "number",
      value: "+14155550188",
      tier: "trusted",
      label: "Clinic",
      created_at: 2,
    },
    {
      id: "row-3",
      agent_id: null,
      subject: "domain",
      value: "clinic.test",
      tier: "trusted",
      label: "Clinic mail",
      created_at: 3,
    },
  ],
  own_addresses: [{ address: "owner@example.com", connection_alias: "mail" }],
  keypad_code: { configured: false, failed_attempts: 0, suspended_until: null },
};

function stubApi(overrides: Record<string, unknown> = {}) {
  return {
    GET: vi.fn(async () => ({ data: page })),
    POST: vi.fn(async () => ({ data: {} })),
    PUT: vi.fn(async () => ({ data: { configured: true } })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
    ...overrides,
  };
}

function mount(api: ReturnType<typeof stubApi>) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <TrustedContacts api={api as unknown as ApiClient} />
    </QueryClientProvider>,
  );
}

describe("TrustedContacts", () => {
  it("opens with the title, the lead and the keypad code before the lists", async () => {
    mount(stubApi());

    expect(
      screen.getByRole("heading", { name: "Trusted contacts" }),
    ).toBeTruthy();
    expect(screen.getByText(/never whether pagis answers/i)).toBeTruthy();
    const labels = screen
      .getAllByTestId("section-label")
      .map((el) => el.textContent);
    expect(labels).toEqual(["Keypad code", "Phone numbers", "Email senders"]);
    const code = await screen.findByTestId("keypad-code");
    expect(code.textContent).toMatch(/No code is set/i);
    expect(
      within(code).queryByRole("button", { name: "Delete the keypad code" }),
    ).toBeNull();
  });

  it("lists numbers and senders in their own frames with a tier chip", async () => {
    mount(stubApi());

    const home = await screen.findByTestId("trust-row-row-1");
    const numbers = screen.getByTestId("trust-list-numbers");
    const senders = screen.getByTestId("trust-list-senders");
    expect(home.textContent).toMatch(/Home/);
    expect(home.textContent).toMatch(/\+1 415 555 0123/);
    expect(within(home).getByText("Owner").className).toMatch(
      /ui-badge-accent/,
    );
    expect(
      within(screen.getByTestId("trust-row-row-2")).getByText("Trusted")
        .className,
    ).toMatch(/ui-badge-working/);
    expect(numbers.textContent).not.toMatch(/clinic\.test/);
    expect(senders.textContent).toMatch(/clinic\.test/);
    expect(senders.textContent).toMatch(/a domain covers every address at it/);
    expect(senders.textContent).not.toMatch(/\+1 415 555 0123/);
  });

  it("shows a connection address as owner with no way to remove it", async () => {
    mount(stubApi());

    const row = await screen.findByTestId("own-address-owner@example.com");
    expect(row.textContent).toMatch(/Your mail account/);
    expect(row.textContent).toMatch(/Owner/);
    expect(within(row).queryByRole("button")).toBeNull();
  });

  it("adds a number from the inline row as trusted by default", async () => {
    const api = stubApi();
    mount(api);
    const numbers = await screen.findByTestId("trust-list-numbers");

    fireEvent.change(
      within(numbers).getByLabelText("Label for the new number"),
      {
        target: { value: "Work" },
      },
    );
    fireEvent.change(within(numbers).getByLabelText("New number"), {
      target: { value: "+14155550199" },
    });
    fireEvent.click(within(numbers).getByRole("button", { name: "Add" }));

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith("/api/v1/settings/trust-list", {
        body: { value: "+14155550199", tier: "trusted", label: "Work" },
      }),
    );
  });

  it("keeps Add off until a value is typed", async () => {
    mount(stubApi());
    const senders = await screen.findByTestId("trust-list-senders");

    expect(
      within(senders)
        .getByRole("button", { name: "Add" })
        .hasAttribute("disabled"),
    ).toBe(true);
  });

  it("removes a listed contact", async () => {
    const api = stubApi();
    mount(api);
    await screen.findByText("+1 415 555 0123");

    fireEvent.click(screen.getByLabelText("Remove +14155550123"));

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        "/api/v1/settings/trust-list/{trust_entry_id}",
        { params: { path: { trust_entry_id: "row-1" } } },
      ),
    );
  });

  it("saves the keypad code", async () => {
    const api = stubApi();
    mount(api);
    await screen.findByTestId("keypad-code");

    fireEvent.change(screen.getByLabelText("Keypad code"), {
      target: { value: "246813" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith("/api/v1/settings/keypad-code", {
        body: { code: "246813" },
      }),
    );
  });

  it("offers Delete only once a code is set", async () => {
    const api = stubApi({
      GET: vi.fn(async () => ({
        data: {
          ...page,
          keypad_code: {
            configured: true,
            failed_attempts: 0,
            suspended_until: null,
          },
        },
      })),
    });
    mount(api);

    await screen.findByText(/A code is set/i);
    const code = screen.getByTestId("keypad-code");
    fireEvent.click(
      within(code).getByRole("button", { name: "Delete the keypad code" }),
    );

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith("/api/v1/settings/keypad-code"),
    );
  });

  it("offers no clear while callers entered no wrong code", async () => {
    mount(stubApi());

    const code = await screen.findByTestId("keypad-code");
    expect(code.textContent).not.toMatch(/wrong code/i);
    expect(
      within(code).queryByRole("button", {
        name: "Clear the failed attempts",
      }),
    ).toBeNull();
  });

  it("shows the failed attempts and the end of the delay, and clears them", async () => {
    // Noon, so the end of the delay falls on the same day and reads as a
    // clock time with no date.
    vi.useFakeTimers({ toFake: ["Date"] });
    vi.setSystemTime(new Date(2026, 0, 15, 12, 0));
    onTestFinished(() => { vi.useRealTimers() });
    const until = Date.now() + 120_000;
    const api = stubApi({
      GET: vi.fn(async () => ({
        data: {
          ...page,
          keypad_code: {
            configured: true,
            failed_attempts: 7,
            suspended_until: until,
          },
        },
      })),
    });
    mount(api);

    const code = await screen.findByTestId("keypad-code");
    await waitFor(() =>
      expect(code.textContent).toMatch(
        /Callers entered a wrong code 7 times\./,
      ),
    );
    expect(code.textContent).toContain(
      `Pagis checks no code until ${formatClock(until)}.`,
    );
    fireEvent.click(
      within(code).getByRole("button", { name: "Clear the failed attempts" }),
    );

    await waitFor(() =>
      expect(api.DELETE).toHaveBeenCalledWith(
        "/api/v1/settings/keypad-code/failures",
      ),
    );
  });

  it("closes with the tier explainer", async () => {
    mount(stubApi());

    const hint = await screen.findByTestId("tier-explainer");
    expect(hint.textContent).toMatch(/Owner speaks as you/);
    expect(hint.textContent).toMatch(/Trusted is believed but cannot approve/);
    expect(hint.textContent).toMatch(/Unknown is read as foreign text/);
  });
});
