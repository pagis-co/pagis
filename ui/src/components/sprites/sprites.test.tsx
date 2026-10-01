import { defaultAppearance } from "../../avatars/catalog";
// The sprites board: the roster reads every non-archived agent, and every
// agent action works from the profile.

import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";

import type { ApiClient } from "../../api/client";
import { usePresence } from "../../state/presence";
import { AgentProfile } from "./AgentProfile";
import { SpriteRoster } from "./SpriteRoster";

const sage = {
  id: "ag1",
  avatar: defaultAppearance(),
  name: "Sage",
  job: "general assistant",
  description: "Ask Sage for anything with no other owner.",
  personality: "warm",
  status: "active",
  email_address: "sage@example.com",
};
const rex = {
  id: "ag2",
  avatar: defaultAppearance(),
  name: "Rex",
  job: "researcher",
  description: "",
  personality: "curious",
  status: "archived",
};

/** Every stub is cast to the client, so the shape is the test's own
 *  business. */
function stubApi(
  items: unknown[],
  numbers: unknown[] = [],
  chief: string | null = "ag1",
) {
  return {
    GET: vi.fn(async (path: string) => {
      if (path === "/api/v1/agents") return { data: { items } };
      if (path === "/api/v1/workspace") {
        return {
          data: {
            id: "ws1",
            name: "Workspace",
            timezone: "UTC",
            chief_of_staff_agent_id: chief,
          },
        };
      }
      if (path === "/api/v1/settings/phone-numbers")
        return { data: { items: numbers } };
      if (path === "/api/v1/settings/onboarding") {
        return {
          data: { completed: true, docker: { endpoint: 'unix:///var/run/docker.sock' as string | null, candidates: [] }, docker_endpoint: null, providers: [] },
        };
      }
      if (path === "/api/v1/channels") {
        return {
          data: {
            items: [
              { id: "ch1", kind: "dm", agent_ids: ["ag1"], user_member: true },
              { id: "ch3", kind: "dm", agent_ids: ["ag3"], user_member: true },
            ],
          },
        };
      }
      if (path === "/api/v1/agents/{agent_id}/computer") {
        return { data: { state: "off", percent: null, holder: "agent" } };
      }
      if (path === "/api/v1/memory/pages/counts") {
        return { data: { pages: 0, procedures: 0, authors: [] } };
      }
      return { data: { items: [] } };
    }),
    POST: vi.fn(async () => ({ data: { id: "ag3" } })),
    PUT: vi.fn(async () => ({ data: {} })),
    DELETE: vi.fn(async () => ({ error: undefined, response: { ok: true } })),
  };
}

function mount(element: React.ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  render(
    <QueryClientProvider client={queryClient}>{element}</QueryClientProvider>,
  );
}

beforeEach(() => {
  usePresence.setState({ runs: {}, onCall: {}, unread: {} });
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => new Response("missing", { status: 404 })),
  );
});

/** `/sprites?new=1` holds the hiring flow; this stands in for it. */
function RosterHarness({
  api,
  onOpenChannel = () => {},
}: {
  api: ApiClient;
  onOpenChannel?: (channelId: string) => void;
}) {
  const [creating, setCreating] = useState(false);
  return (
    <SpriteRoster
      api={api}
      creating={creating}
      onCreating={setCreating}
      onOpenAgent={() => {}}
      onOpenChannel={onOpenChannel}
    />
  );
}

describe("the sprite roster", () => {
  it("shows a load error instead of claiming that there are no agents", async () => {
    const api = stubApi([]);
    api.GET.mockRejectedValue(new Error("Unavailable"));
    mount(<RosterHarness api={api as unknown as ApiClient} />);
    expect(await screen.findByText("Could not load your sprites")).toBeTruthy();
    expect(screen.queryByText(/No agent yet/)).toBeNull();
  });

  it("lists every non-archived agent with presence and the lines it answers on", async () => {
    mount(
      <RosterHarness
        api={
          stubApi(
            [sage, rex],
            [{ agent_id: "ag1", e164: "+14155550123" }],
          ) as unknown as ApiClient
        }
      />,
    );

    expect(await screen.findByText("Sage")).toBeTruthy();
    expect(screen.queryByText("Rex")).toBeNull();
    expect(screen.getByText("Idle")).toBeTruthy();
    expect(screen.getByText("sage@example.com")).toBeTruthy();
    expect(screen.getByText("+1 415 555 0123")).toBeTruthy();
  });

  it("marks the Chief of Staff the Workspace names", async () => {
    const clown = { ...sage, id: "ag4", name: "Clown", job: "jester" };
    mount(
      <RosterHarness
        api={stubApi([sage, clown], [], "ag4") as unknown as ApiClient}
      />,
    );

    const row = await screen.findByRole("button", { name: /^Clown,/ });
    expect(within(row).getByText("Chief of Staff")).toBeTruthy();
    const other = screen.getByRole("button", { name: /^Sage,/ });
    expect(within(other).queryByText("Chief of Staff")).toBeNull();
  });

  // The ring stands for the work of a conversation (ADR-0022), so the
  // Run the roster reads has a Channel.
  it("says the presence of an agent that is working", async () => {
    usePresence.setState({
      runs: {
        "run-1": {
          runId: "run-1",
          agentId: "ag1",
          channelId: "ch1",
          originChannelId: null,
          state: "running",
          caption: "Working",
        },
      },
    });
    mount(<RosterHarness api={stubApi([sage]) as unknown as ApiClient} />);

    expect(await screen.findByText("Working")).toBeTruthy();
  });

  it("opens the agent that a row names", async () => {
    const opened: string[] = [];
    mount(
      <SpriteRoster
        api={stubApi([sage]) as unknown as ApiClient}
        creating={false}
        onCreating={() => {}}
        onOpenAgent={(agentId) => opened.push(agentId)}
        onOpenChannel={() => {}}
      />,
    );

    fireEvent.click(await screen.findByTestId("sprite-row"));
    expect(opened).toEqual(["ag1"]);
  });
});

/** The four steps of hiring, from the roster button to Hire. */
async function walkToAccess(name = "Rex", job = "researcher", hue?: string) {
  fireEvent.click(await screen.findByText("Hire a sprite"));
  fireEvent.change(await screen.findByLabelText("Sprite name"), {
    target: { value: name },
  });
  if (hue) fireEvent.click(screen.getByRole("radio", { name: hue }));
  fireEvent.click(screen.getByText("Next"));
  fireEvent.change(await screen.findByLabelText("Sprite job"), {
    target: { value: job },
  });
  fireEvent.change(await screen.findByLabelText("Sprite description"), {
    target: { value: "Ask Rex to look things up." },
  });
  fireEvent.click(screen.getByText("Next"));
  fireEvent.change(await screen.findByLabelText("Sprite personality"), {
    target: { value: "curious" },
  });
  fireEvent.click(screen.getByText("Next"));
  await screen.findByTestId("hire-step-access");
}

describe("hiring an agent", () => {
  it("walks the four steps and lands in the new agent DM", async () => {
    const api = stubApi([sage]);
    const opened: string[] = [];
    mount(
      <RosterHarness
        api={api as unknown as ApiClient}
        onOpenChannel={(channelId) => opened.push(channelId)}
      />,
    );

    await walkToAccess("Rex", "researcher", "Lavender");
    fireEvent.click(screen.getByText("Hire"));

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith("/api/v1/agents", {
        body: {
          name: "Rex",
          job: "researcher",
          description: "Ask Rex to look things up.",
          personality: "curious",
          voice: null,
          avatar: { ...defaultAppearance(), preset: "lavender" },
        },
      }),
    );
    // The daemon makes the DM with the agent; hiring finds it.
    await waitFor(() => expect(opened).toEqual(["ch3"]));
  });

  it("grants the connections the access step picks", async () => {
    const api = stubApi([sage]);
    api.GET.mockImplementation(async (path: string) => {
      if (path === "/api/v1/settings/connections") {
        return {
          data: {
            items: [
              {
                id: "conn1",
                provider: "google",
                display_name: "Google",
                alias: "work",
                account: "ada@example.com",
                capabilities: ["mail"],
                authorized_capabilities: ["gmail_read"],
                status: "connected",
                auth_mode: "byo",
                created_at: 0,
              },
            ],
          },
        };
      }
      if (path === "/api/v1/agents") return { data: { items: [sage] } };
      if (path === "/api/v1/channels") {
        return {
          data: {
            items: [
              { id: "ch3", kind: "dm", agent_ids: ["ag3"], user_member: true },
            ],
          },
        };
      }
      return { data: { items: [] } };
    });
    mount(<RosterHarness api={api as unknown as ApiClient} />);

    await walkToAccess();
    fireEvent.click(screen.getByLabelText("Read Gmail"));
    fireEvent.click(screen.getByText("Hire"));

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith("/api/v1/grants", {
        body: {
          agent_id: "ag3",
          connection_id: "conn1",
          capabilities: ["gmail_read"],
        },
      }),
    );
  });

  it("holds each step until its own fields are given", async () => {
    const api = stubApi([sage]);
    mount(<RosterHarness api={api as unknown as ApiClient} />);

    fireEvent.click(await screen.findByText("Hire a sprite"));
    fireEvent.click(await screen.findByText("Next"));
    expect(await screen.findByText("Give the sprite a name.")).toBeTruthy();
    expect(screen.getByTestId("hire-step-face")).toBeTruthy();

    fireEvent.change(screen.getByLabelText("Sprite name"), {
      target: { value: "Rex" },
    });
    fireEvent.click(screen.getByText("Next"));
    fireEvent.click(await screen.findByText("Next"));
    expect(await screen.findByText("Say what this sprite does.")).toBeTruthy();
    expect(screen.getByTestId("hire-step-job")).toBeTruthy();
  });

  it("keeps every entry when the user goes back", async () => {
    const api = stubApi([sage]);
    mount(<RosterHarness api={api as unknown as ApiClient} />);

    await walkToAccess();
    fireEvent.click(screen.getByText("Back"));
    expect(
      (await screen.findByLabelText<HTMLTextAreaElement>("Sprite personality"))
        .value,
    ).toBe("curious");
    fireEvent.click(screen.getByText("Back"));
    expect(
      (await screen.findByLabelText<HTMLInputElement>("Sprite job")).value,
    ).toBe("researcher");
    fireEvent.click(screen.getByText("Back"));
    expect(
      (await screen.findByLabelText<HTMLInputElement>("Sprite name")).value,
    ).toBe("Rex");
  });
});

function profile(api: unknown, onOpenMemory = vi.fn()) {
  mount(
    <AgentProfile
      api={api as unknown as ApiClient}
      agentId="ag1"
      onBack={() => {}}
      onOpenRun={() => {}}
      onOpenMemory={onOpenMemory}
      onOpenSyncSettings={() => {}}
    />,
  );
}

describe("the agent profile", () => {
  it("saves a sprite style and keeps a failed save available for retry", async () => {
    const api = stubApi([sage]);
    api.PUT.mockResolvedValueOnce({
      error: { message: "Save failed" },
      response: { ok: false },
    } as never);
    profile(api);
    await userEvent.click(
      await screen.findByRole("tab", { name: "Appearance" }),
    );
    fireEvent.click(screen.getByRole("radio", { name: "Lavender" }));
    fireEvent.input(screen.getByLabelText("Body color"), {
      target: { value: "#123456" },
    });
    fireEvent.click(screen.getByRole("checkbox", { name: "Satchel" }));
    fireEvent.click(screen.getByRole("button", { name: "Save appearance" }));
    await screen.findByRole("alert");
    expect(
      (screen.getByLabelText("Body color") as HTMLInputElement).value,
    ).toBe("#123456");
    api.PUT.mockResolvedValueOnce({
      data: {
        ...sage,
        avatar: {
          ...defaultAppearance(),
          preset: "lavender",
          colors: { body: "#123456" },
          accessories: { satchel: false },
        },
      },
    });
    fireEvent.click(screen.getByRole("button", { name: "Save appearance" }));
    await screen.findByText("Appearance saved.");
    expect(api.PUT).toHaveBeenLastCalledWith(
      "/api/v1/agents/{agent_id}/appearance",
      {
        params: { path: { agent_id: "ag1" } },
        body: {
          sprite: "pixie",
          preset: "lavender",
          colors: { body: "#123456" },
          accessories: { satchel: false },
        },
      },
    );
  });

  it("edits the agent in place", async () => {
    const api = stubApi([sage]);
    profile(api);

    fireEvent.click(await screen.findByLabelText("Edit Sage"));
    fireEvent.change(screen.getByLabelText("Sprite job"), {
      target: { value: "chief of staff" },
    });
    fireEvent.change(screen.getByLabelText("Sprite description"), {
      target: { value: "Ask Sage to run the day." },
    });
    fireEvent.click(screen.getByText("Save"));

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith("/api/v1/agents/{agent_id}", {
        params: { path: { agent_id: "ag1" } },
        body: {
          name: "Sage",
          job: "chief of staff",
          description: "Ask Sage to run the day.",
          personality: "warm",
          voice: null,
        },
      }),
    );
  });

  it("archives the agent", async () => {
    const api = stubApi([sage]);
    profile(api);

    fireEvent.click(await screen.findByLabelText("Archive Sage"));

    await waitFor(() =>
      expect(api.POST).toHaveBeenCalledWith(
        "/api/v1/agents/{agent_id}/archive",
        {
          params: { path: { agent_id: "ag1" } },
        },
      ),
    );
  });

  it("leaves an archived agent no action", async () => {
    profile(stubApi([{ ...sage, status: "archived" }]));

    expect(await screen.findByTestId("agent-profile")).toBeTruthy();
    expect(screen.queryByLabelText("Edit Sage")).toBeNull();
    expect(screen.queryByLabelText("Archive Sage")).toBeNull();
  });

  it("carries every agent section on the profile", async () => {
    profile(stubApi([sage]));

    await screen.findByRole("heading", { name: "Sage" });
    for (const label of [
      "About",
      "Desk",
      "Contact",
      "Access",
      "Memory",
      "Work",
    ]) {
      expect(screen.getByRole("tab", { name: label })).toBeTruthy();
    }
  });

  it("shows the memory summary on the Memory tab", async () => {
    const onOpenMemory = vi.fn();
    profile(stubApi([sage]), onOpenMemory);

    await userEvent.click(await screen.findByRole("tab", { name: "Memory" }));

    expect(await screen.findByText("Holds")).toBeTruthy();
    await userEvent.click(
      screen.getByRole("button", { name: "Open in Memory" }),
    );
    expect(onOpenMemory).toHaveBeenCalledWith({
      scope: "agent:ag1",
      view: "pages",
    });
  });

  it("makes the agent the Chief of Staff", async () => {
    const api = stubApi([sage], [], "ag4");
    profile(api);

    await userEvent.click(
      await screen.findByRole("button", {
        name: "Make Sage the Chief of Staff",
      }),
    );

    await waitFor(() =>
      expect(api.PUT).toHaveBeenCalledWith("/api/v1/workspace/chief-of-staff", {
        body: { agent_id: "ag1" },
      }),
    );
  });

  it("says that the agent already is the Chief of Staff", async () => {
    profile(stubApi([sage], [], "ag1"));

    expect(await screen.findByTestId("agent-chief-of-staff")).toBeTruthy();
    expect(
      screen.queryByRole("button", { name: "Make Sage the Chief of Staff" }),
    ).toBeNull();
  });

  it("marks the Chief of Staff in the profile header", async () => {
    profile(stubApi([sage], [], "ag1"));

    const header = (
      await screen.findByRole("heading", { name: "Sage" })
    ).closest("header") as HTMLElement;
    expect(await within(header).findByText("Chief of Staff")).toBeTruthy();
  });

  it("leaves the Chief of Staff badge off another sprite's header", async () => {
    profile(stubApi([sage], [], "ag4"));

    const header = (
      await screen.findByRole("heading", { name: "Sage" })
    ).closest("header") as HTMLElement;
    await screen.findByRole("button", { name: "Make Sage the Chief of Staff" });
    expect(within(header).queryByText("Chief of Staff")).toBeNull();
  });

  it("shows the Docker hint on the Desk when Docker is absent", async () => {
    const api = stubApi([sage]);
    api.GET.mockImplementation(async (path: string) => {
      if (path === "/api/v1/agents") return { data: { items: [sage] } };
      if (path === "/api/v1/settings/onboarding") {
        return {
          data: {
            completed: true,
            docker: { endpoint: null, candidates: [] },
            docker_endpoint: null,
            providers: [],
          },
        };
      }
      if (path === "/api/v1/agents/{agent_id}/computer") {
        return { data: { state: "off", percent: null, holder: "agent" } };
      }
      return { data: { items: [] } };
    });
    profile(api);

    await userEvent.click(await screen.findByRole("tab", { name: "Desk" }));

    expect(await screen.findByText(/found no Docker/)).toBeTruthy();
  });

  it("shows a Google hint on Access when no Google account is connected", async () => {
    profile(stubApi([sage]));

    await userEvent.click(await screen.findByRole("tab", { name: "Access" }));

    expect(await screen.findByTestId("agent-access-no-google")).toBeTruthy();
  });
});
