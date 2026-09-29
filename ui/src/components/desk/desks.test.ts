import { defaultAppearance } from "../../avatars/catalog";
// Which Desks the panel shows, and in what order (ADR-0022).

import { describe, expect, it } from "vitest";

import type { AgentDto } from "../../api/client";
import type { LiveRun } from "../../state/presence";
import { deskRows } from "./desks";

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
    job: "jester",
    description: "",
    avatar: defaultAppearance(),
    personality: "",
    status: "active",
  },
  {
    id: "ag-3",
    name: "Nurse",
    job: "carer",
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

function run(overrides: Partial<LiveRun>): LiveRun {
  return {
    runId: "run-1",
    agentId: "ag-2",
    channelId: "ch-clown",
    originChannelId: null,
    state: "running",
    caption: "Working",
    ...overrides,
  };
}

describe("deskRows on Home", () => {
  it("puts the Chief of Staff first and every other active Agent after it", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-2",
      surface: "home",
      channelId: null,
      runs: [],
    });

    expect(rows.chief?.id).toBe("ag-2");
    expect(rows.others.map((agent) => agent.id)).toEqual(["ag-1", "ag-3"]);
  });

  it("leaves an archived Agent out: an archived Agent has no Desk", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-1",
      surface: "home",
      channelId: null,
      runs: [],
    });

    expect(rows.others.map((agent) => agent.id)).toEqual(["ag-2", "ag-3"]);
  });

  it("shows every Desk even when the Workspace names no Chief of Staff", () => {
    const rows = deskRows({
      agents,
      chiefId: null,
      surface: "home",
      channelId: null,
      runs: [],
    });

    expect(rows.chief).toBeNull();
    expect(rows.others.map((agent) => agent.id)).toEqual([
      "ag-1",
      "ag-2",
      "ag-3",
    ]);
  });
});

describe("deskRows in the direct channel", () => {
  it("shows the Chief of Staff alone", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-1",
      surface: "channel",
      channelId: "ch-sage",
      runs: [],
    });

    expect(rows.chief?.id).toBe("ag-1");
    expect(rows.others).toEqual([]);
  });

  it("adds the Desk of an Agent working on a Delegation from that channel", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-1",
      surface: "channel",
      channelId: "ch-sage",
      runs: [run({ agentId: "ag-2", originChannelId: "ch-sage" })],
    });

    expect(rows.others.map((agent) => agent.id)).toEqual(["ag-2"]);
  });

  it("leaves out a Delegation another channel is waiting on", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-1",
      surface: "channel",
      channelId: "ch-sage",
      runs: [run({ agentId: "ag-2", originChannelId: "ch-other" })],
    });

    expect(rows.others).toEqual([]);
  });

  it("leaves out a Run of the Chief of Staff itself, which already has the first Desk", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-1",
      surface: "channel",
      channelId: "ch-sage",
      runs: [run({ agentId: "ag-1", originChannelId: "ch-sage" })],
    });

    expect(rows.others).toEqual([]);
  });

  it("names each delegate once, however many Runs it has", () => {
    const rows = deskRows({
      agents,
      chiefId: "ag-1",
      surface: "channel",
      channelId: "ch-sage",
      runs: [
        run({ runId: "run-1", agentId: "ag-2", originChannelId: "ch-sage" }),
        run({ runId: "run-2", agentId: "ag-2", originChannelId: "ch-sage" }),
      ],
    });

    expect(rows.others.map((agent) => agent.id)).toEqual(["ag-2"]);
  });
});
