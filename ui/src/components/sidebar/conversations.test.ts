import { defaultAppearance } from "../../avatars/catalog";
// The order of the conversations: the Chief of Staff first, the
// other Agents next, the groups last. The Workspace names the Chief of
// Staff (ADR-0022), so the reading order follows the setting.

import { describe, expect, it } from "vitest";

import type { AgentDto, ChannelDto } from "../../api/client";
import { chiefOfStaff, conversationRows } from "./conversations";

const agents: AgentDto[] = [
  {
    id: "ag-1",
    name: "Sage",
    job: "general assistant",
    description: "",
    avatar: defaultAppearance(),
    personality: "",
    status: "active",
  },
  {
    id: "ag-2",
    name: "Clown",
    job: "Chief Entertainment Officer",
    description: "",
    avatar: defaultAppearance(),
    personality: "",
    status: "active",
  },
  {
    id: "ag-0",
    name: "Gone",
    job: "retired",
    description: "",
    avatar: defaultAppearance(),
    personality: "",
    status: "archived",
  },
];

function channel(overrides: Partial<ChannelDto>): ChannelDto {
  return {
    id: "ch",
    workspace_id: "ws-1",
    kind: "dm",
    agent_ids: [],
    user_member: true,
    title: null,
    created_at: 1,
    updated_at: 1,
    ...overrides,
  };
}

// The server order is not the reading order.
const channels = [
  channel({
    id: "ch-group",
    kind: "group",
    agent_ids: ["ag-1", "ag-2"],
    title: "Sage ↔ Clown",
  }),
  channel({ id: "ch-clown", agent_ids: ["ag-2"], title: "Clown" }),
  channel({ id: "ch-sage", agent_ids: ["ag-1"], title: "Sage" }),
];

describe("chiefOfStaff", () => {
  it("names the Agent the Workspace names", () => {
    expect(chiefOfStaff(agents, "ag-2")?.id).toBe("ag-2");
  });

  it("names nobody before the Workspace is read", () => {
    expect(chiefOfStaff(agents, undefined)).toBeNull();
  });

  it("names nobody when the Workspace names nobody", () => {
    expect(chiefOfStaff(agents, null)).toBeNull();
  });

  it("names nobody when the named Agent is archived", () => {
    expect(chiefOfStaff(agents, "ag-0")).toBeNull();
  });

  it("names nobody when the named Agent is off the roster", () => {
    expect(chiefOfStaff(agents, "ag-9")).toBeNull();
  });
});

describe("conversationRows", () => {
  it("puts the Chief of Staff first, then the Agents, then the groups", () => {
    const rows = conversationRows(channels, agents, "ag-1");
    expect(rows.map((row) => row.channel.id)).toEqual([
      "ch-sage",
      "ch-clown",
      "ch-group",
    ]);
    expect(rows.map((row) => row.kind)).toEqual(["chief", "agent", "group"]);
  });

  it("follows the setting when the Workspace names another Agent", () => {
    const rows = conversationRows(channels, agents, "ag-2");
    expect(rows.map((row) => row.channel.id)).toEqual([
      "ch-clown",
      "ch-sage",
      "ch-group",
    ]);
    expect(rows.map((row) => row.kind)).toEqual(["chief", "agent", "group"]);
  });

  it("reads a DM with no roster Agent as a group", () => {
    const rows = conversationRows(
      [channel({ id: "ch-x", agent_ids: ["ag-9"] })],
      agents,
      "ag-1",
    );
    expect(rows[0]?.kind).toBe("group");
  });

  // A channel two Agents opened between themselves is theirs, not the
  // user's own channel with one of them (ADR-0003).
  it("reads a channel without the user as an Agent channel, and lists it last", () => {
    const rows = conversationRows(
      [
        channel({
          id: "ch-agents",
          agent_ids: ["ag-1", "ag-2"],
          user_member: false,
          title: "Sage ↔ Clown",
        }),
        channel({ id: "ch-sage", agent_ids: ["ag-1"], title: "Sage" }),
      ],
      agents,
      "ag-1",
    );
    expect(rows.map((row) => row.kind)).toEqual(["chief", "agents"]);
    expect(rows[1]?.agent).toBeNull();
  });
});
