// One sprite's profile at `/sprites/:agentId`. Six sections, each a
// composition of what the product already has: About (the profile
// form), Desk (the computer tile), Contact (the phone number and the
// mailbox), Access (the grants), Memory
// (a summary of what this Agent holds and learns from) and Work (this Agent's runs).

import { useState } from "react";
import { ChevronLeft } from "lucide-react";

import type { AgentDto, ApiClient } from "../../api/client";
import { Badge, Button, Tabs } from "../../primitives";
import { SpriteAvatar } from "../../avatars/SpriteAvatar";
import { AgentAppearance } from "../../avatars/AgentAppearance";
import { useLiveAvatarMotion } from "../../avatars/liveMotion";
import { useConnection } from "../../state/stores";
import {
  useAgents,
  useArchiveAgent,
  useRuns,
  useSetChiefOfStaff,
  useUpdateAgent,
  useWorkspace,
} from "../../queries";
import { agentPresence, usePresence } from "../../state/presence";
import { AgentMailbox } from "../AgentMailbox";
import { AgentPhoneNumber } from "../AgentPhoneNumber";
import { AgentAccess } from "./AgentAccess";
import { AgentDesk } from "./AgentDesk";
import { AgentForm } from "./AgentForm";
import { AgentMemory, type AgentMemoryProps } from "./AgentMemory";
import { AgentWork } from "./AgentWork";
import { activityTone, activityWord } from "../stateWords";
import { lastActiveAt, lastActiveLabel } from "./labels";

import "../agent.css";
import "../settings.css";
import "./sprites.css";

type Section =
  | "appearance"
  | "about"
  | "desk"
  | "contact"
  | "access"
  | "memory"
  | "work";

const SECTIONS: { value: Section; label: string }[] = [
  { value: "about", label: "About" },
  { value: "appearance", label: "Appearance" },
  { value: "desk", label: "Desk" },
  { value: "contact", label: "Contact" },
  { value: "access", label: "Access" },
  { value: "memory", label: "Memory" },
  { value: "work", label: "Work" },
];

function About({ api, agent }: { api: ApiClient; agent: AgentDto }) {
  const update = useUpdateAgent(api);
  const archive = useArchiveAgent(api);
  const workspace = useWorkspace(api);
  const designate = useSetChiefOfStaff(api);
  const [editing, setEditing] = useState(false);
  const chief = workspace.data?.chief_of_staff_agent_id === agent.id;

  if (editing) {
    return (
      <AgentForm
        api={api}
        initial={{
          name: agent.name,
          job: agent.job,
          description: agent.description,
          personality: agent.personality,
          voice: agent.voice ?? null,
        }}
        submitLabel="Save"
        pending={update.isPending}
        onSubmit={(fields) =>
          update.mutate(
            { agentId: agent.id, ...fields },
            { onSuccess: () => setEditing(false) },
          )
        }
        onCancel={() => setEditing(false)}
      />
    );
  }

  return (
    <section className="agent-about" aria-label={`${agent.name} about`}>
      <dl className="agent-about-facts">
        <dt>Job</dt>
        <dd>{agent.job === "" ? "No job written yet." : agent.job}</dd>
        <dt>What to ask it for</dt>
        <dd>
          {agent.description === ""
            ? "No description written yet. The other sprites read this line."
            : agent.description}
        </dd>
        <dt>Personality</dt>
        <dd>
          {agent.personality === ""
            ? "No personality written yet."
            : agent.personality}
        </dd>
        <dt>Voice</dt>
        <dd>{agent.voice ?? "Provider default voice"}</dd>
      </dl>
      {agent.status === "archived" ? (
        <p className="settings-hint">
          {agent.name} is archived. It runs no more; its history stays.
        </p>
      ) : (
        <div className="settings-row-actions">
          <Button
            aria-label={`Edit ${agent.name}`}
            onClick={() => setEditing(true)}
          >
            Edit
          </Button>
          {!chief && (
            <Button
              aria-label={`Make ${agent.name} the Chief of Staff`}
              disabled={designate.isPending}
              onClick={() => designate.mutate(agent.id)}
            >
              Make Chief of Staff
            </Button>
          )}
          <Button
            variant="danger"
            aria-label={`Archive ${agent.name}`}
            disabled={archive.isPending}
            onClick={() => archive.mutate(agent.id)}
          >
            Archive
          </Button>
        </div>
      )}
      {/* The designation grants nothing; it decides what the shell
          shows first (ADR-0022). */}
      {chief ? (
        <p className="settings-hint" data-testid="agent-chief-of-staff">
          {agent.name} is your Chief of Staff. Home is its report, and the
          composer on Home speaks to it.
        </p>
      ) : (
        agent.status !== "archived" && (
          <p className="settings-hint">
            The Chief of Staff sits at the top of the sidebar and writes the
            report on Home.
          </p>
        )
      )}
      <p className="settings-hint">
        Archiving stops every trigger. The conversations and the work record
        stay.
      </p>
    </section>
  );
}

export function AgentProfile({
  api,
  agentId,
  onBack,
  onOpenRun,
  onOpenMemory,
  onOpenSyncSettings,
}: {
  api: ApiClient;
  agentId: string;
  onBack: () => void;
  onOpenRun: (runId: string) => void;
  onOpenMemory: AgentMemoryProps["onOpenMemory"];
  onOpenSyncSettings: () => void;
}) {
  const agents = useAgents(api);
  const workspace = useWorkspace(api);
  const runs = useRuns(api, agentId, "", "");
  const [section, setSection] = useState<Section>("about");
  const agent = (agents.data ?? []).find((row) => row.id === agentId);
  const presence = usePresence((state) => agentPresence(state, agentId));
  const online = useConnection((state) => state.status === "online");
  const motion = useLiveAvatarMotion(agentId, presence);

  if (agent === undefined) {
    return (
      <div className="agent-profile" data-testid="agent-profile">
        <Button className="agent-profile-back" onClick={onBack}>
          <ChevronLeft size={16} aria-hidden />
          Sprites
        </Button>
        <p className="settings-hint">
          {agents.isPending ? "Loading…" : "That sprite is not one of yours."}
        </p>
      </div>
    );
  }

  return (
    <div className="agent-profile" data-testid="agent-profile">
      <header className="agent-profile-header">
        <Button className="agent-profile-back" onClick={onBack}>
          <ChevronLeft size={16} aria-hidden />
          Sprites
        </Button>
        <SpriteAvatar
          name={agent.name}
          {...motion}
          animate={section !== "appearance" && online}
          appearance={agent.avatar}
        />
        <div className="agent-profile-identity">
          <h2>{agent.name}</h2>
          <span className="agent-job">{agent.job}</span>
        </div>
        {workspace.data?.chief_of_staff_agent_id === agent.id && (
          <Badge tone="neutral">Main sprite</Badge>
        )}
        <Badge tone={activityTone(presence)}>{activityWord(presence)}</Badge>
        <span className="sprite-row-last">
          {lastActiveLabel(lastActiveAt(runs.data ?? [], agent.id))}
        </span>
      </header>
      <Tabs
        label={`${agent.name} profile`}
        value={section}
        onValueChange={(next) => setSection(next as Section)}
        items={SECTIONS}
      >
        {section === "appearance" && (
          <AgentAppearance key={agent.id} api={api} agent={agent} />
        )}
        {section === "about" && <About api={api} agent={agent} />}
        {section === "desk" && <AgentDesk api={api} agent={agent} />}
        {section === "contact" && (
          <section
            className="agent-contact"
            aria-label={`${agent.name} contact`}
          >
            <AgentPhoneNumber api={api} agent={agent} />
            <AgentMailbox api={api} agent={agent} />
          </section>
        )}
        {section === "access" && <AgentAccess api={api} agent={agent} />}
        {section === "memory" && (
          <AgentMemory
            api={api}
            agent={agent}
            onOpenMemory={onOpenMemory}
            onOpenSyncSettings={onOpenSyncSettings}
            onOpenRun={onOpenRun}
          />
        )}
        {section === "work" && (
          <AgentWork
            api={api}
            agentId={agent.id}
            agentName={agent.name}
            onOpenRun={onOpenRun}
          />
        )}
      </Tabs>
    </div>
  );
}
