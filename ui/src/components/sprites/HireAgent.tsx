// "Hire a sprite": four short steps instead of one long form.
// Name and face, job and description, personality and voice, then the access the new
// Agent starts with. The draft lives in this component, so Back keeps
// every entry, and each step answers for its own fields.
//
// The last step writes in one go: the Agent (with its Mailbox, ADR-0019),
// then one Connection grant per picked capability set, then the flow
// lands in the Agent's DM. The daemon makes the DM with the Agent
// (agents.rs), so the flow only has to find it.

import { useEffect, useState } from "react";

import type { ApiClient } from "../../api/client";
import {
  Avatar,
  Button,
  Dialog,
  Input,
  Textarea,
} from "../../primitives";
import {
  errorMessage,
  useConnections,
  useCreateAgent,
  useCreateConnectionGrant,
  useMailboxOffers,
  type NewMailboxBody,
} from "../../queries";
import { CapabilityPicker } from "../ConnectionCapabilities";
import { VoicePicker } from "./VoicePicker";
import {
  MailboxFields,
  NoMailboxProvider,
  draftIsReady,
  mailboxBody,
  offerOf,
  type MailboxDraft,
  type MailboxOffer,
} from "../AgentMailbox";

import { AppearanceControls } from "../../avatars/AppearanceControls";
import {
  defaultAppearance,
  type SpriteAppearance,
} from "../../avatars/catalog";

import "../agent.css";
import "../settings.css";
import "./sprites.css";

const STEPS = ["face", "job", "voice", "access"] as const;
type Step = (typeof STEPS)[number];

const STEP_TITLE: Record<Step, string> = {
  face: "Name and face",
  job: "What is the job?",
  voice: "Personality and voice",
  access: "What may it reach?",
};

/** What the four steps fill in. One object, so Back loses nothing. */
interface HireDraft {
  avatar: SpriteAppearance;
  name: string;
  job: string;
  description: string;
  personality: string;
  voice: string;
  /** Capabilities per Connection id; an empty list grants nothing. */
  capabilities: Record<string, string[]>;
  mailboxWanted: boolean;
  mailbox: MailboxDraft;
}

const EMPTY_DRAFT: HireDraft = {
  avatar: defaultAppearance(),
  name: "",
  job: "",
  description: "",
  personality: "",
  voice: "",
  capabilities: {},
  mailboxWanted: false,
  mailbox: { connectionId: "", localPart: "", outgoingCap: "", password: "" },
};

/** What one step refuses to leave with, or `null` when it is ready. */
function stepError(
  step: Step,
  draft: HireDraft,
  offer?: MailboxOffer,
): string | null {
  if (step === "face" && draft.name.trim() === "") {
    return "Give the sprite a name.";
  }
  if (step === "job" && draft.job.trim() === "") {
    return "Say what this sprite does.";
  }
  if (
    step === "access" &&
    draft.mailboxWanted &&
    !draftIsReady(draft.mailbox, offer)
  ) {
    return "Finish the mailbox, or leave it out.";
  }
  return null;
}

export function HireAgent({
  api,
  open,
  onCancel,
  onHired,
}: {
  api: ApiClient;
  open: boolean;
  onCancel: () => void;
  /** The new Agent's DM, so the flow ends where the work starts. */
  onHired: (channelId: string) => void;
}) {
  const [step, setStep] = useState<Step>("face");
  const [draft, setDraft] = useState<HireDraft>(EMPTY_DRAFT);
  const [shown, setShown] = useState<string | null>(null);
  const [failure, setFailure] = useState<unknown>(null);
  const [hiring, setHiring] = useState(false);

  const connections = useConnections(api);
  const createAgent = useCreateAgent(api);
  const createGrant = useCreateConnectionGrant(api);
  const offers = (useMailboxOffers(api, draft.name, open).data ??
    []) as MailboxOffer[];
  const [namedMailbox, setNamedMailbox] = useState(false);

  // The suggested address follows the Agent's name until the user
  // writes one of their own.
  const suggestion = offers[0];
  useEffect(() => {
    if (namedMailbox || suggestion === undefined) return;
    setDraft((current) =>
      current.mailbox.connectionId === suggestion.connection_id &&
      current.mailbox.localPart === suggestion.suggested_local_part
        ? current
        : {
            ...current,
            mailbox: {
              ...current.mailbox,
              connectionId: suggestion.connection_id,
              localPart: suggestion.suggested_local_part,
            },
          },
    );
  }, [namedMailbox, suggestion]);

  const offer = offerOf(offers, draft.mailbox.connectionId);
  const patch = (next: Partial<HireDraft>) => {
    setShown(null);
    setDraft((current) => ({ ...current, ...next }));
  };

  const close = () => {
    setStep("face");
    setDraft(EMPTY_DRAFT);
    setNamedMailbox(false);
    setShown(null);
    setFailure(null);
    onCancel();
  };

  const advance = () => {
    const error = stepError(step, draft, offer);
    if (error !== null) {
      setShown(error);
      return;
    }
    setShown(null);
    setStep(STEPS[STEPS.indexOf(step) + 1]);
  };

  const back = () => {
    setShown(null);
    setStep(STEPS[STEPS.indexOf(step) - 1]);
  };

  const hire = async () => {
    const error = stepError("access", draft, offer);
    if (error !== null) {
      setShown(error);
      return;
    }
    setFailure(null);
    setHiring(true);
    try {
      const mailbox: NewMailboxBody | undefined =
        draft.mailboxWanted && offer !== undefined
          ? mailboxBody(draft.mailbox, offer)
          : undefined;
      const agent = await createAgent.mutateAsync({
        avatar: draft.avatar,
        name: draft.name.trim(),
        job: draft.job.trim(),
        description: draft.description.trim(),
        personality: draft.personality.trim(),
        voice: draft.voice === "" ? null : draft.voice,
        mailbox,
      });
      for (const [connectionId, capabilities] of Object.entries(
        draft.capabilities,
      )) {
        if (capabilities.length === 0) continue;
        await createGrant.mutateAsync({
          agent_id: agent.id,
          connection_id: connectionId,
          capabilities,
        });
      }
      const { data } = await api.GET("/api/v1/channels");
      // The new agent's own channel with the user (ADR-0003).
      const dm = (data?.items ?? []).find(
        (channel) =>
          channel.kind === "dm" &&
          channel.user_member &&
          channel.agent_ids.includes(agent.id),
      );
      if (dm === undefined) throw new Error("the new sprite has no DM yet");
      close();
      onHired(dm.id);
    } catch (error) {
      setFailure(error);
    } finally {
      setHiring(false);
    }
  };

  const index = STEPS.indexOf(step);
  const last = index === STEPS.length - 1;

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (!next) close();
      }}
      title="Hire a sprite"
      description={`Step ${index + 1} of ${STEPS.length}: ${STEP_TITLE[step]}`}
      footer={
        <div className="hire-actions">
          {index > 0 && (
            <Button type="button" onClick={back} disabled={hiring}>
              Back
            </Button>
          )}
          {last ? (
            <Button
              type="button"
              variant="primary"
              disabled={hiring}
              onClick={() => void hire()}
            >
              Hire
            </Button>
          ) : (
            <Button type="button" variant="primary" onClick={advance}>
              Next
            </Button>
          )}
        </div>
      }
    >
      <div className="hire-step" data-testid={`hire-step-${step}`}>
        {step === "face" && (
          <>
            <Avatar
              id={draft.name}
              name={draft.name}
              appearance={draft.avatar}
              size="lg"
            />
            <AppearanceControls
              compact
              value={draft.avatar}
              onChange={(avatar) => patch({ avatar })}
            />
            <Input
              aria-label="Sprite name"
              placeholder="Name"
              value={draft.name}
              onChange={(event) => patch({ name: event.target.value })}
            />
          </>
        )}
        {step === "job" && (
          <>
            <p className="settings-hint">
              The job is the first thing the sprite reads about itself. The
              description is what the other sprites read, so they know what to
              ask this one for.
            </p>
            <Input
              aria-label="Sprite job"
              placeholder="Job, e.g. research assistant"
              value={draft.job}
              onChange={(event) => patch({ job: event.target.value })}
            />
            <Input
              aria-label="Sprite description"
              placeholder="What to ask this sprite for"
              value={draft.description}
              onChange={(event) => patch({ description: event.target.value })}
            />
          </>
        )}
        {step === "voice" && (
          <>
            <p className="settings-hint">
              How it writes, and how it sounds on a call. Both are optional.
            </p>
            <Textarea
              aria-label="Sprite personality"
              placeholder="Personality"
              value={draft.personality}
              onChange={(event) => patch({ personality: event.target.value })}
            />
            <VoicePicker
              api={api}
              value={draft.voice}
              onChange={(voice) => patch({ voice })}
            />
          </>
        )}
        {step === "access" && (
          <>
            <p className="settings-hint">
              Pick what the sprite may reach on day one. You can change every
              grant later from its Access section.
            </p>
            {(connections.data ?? []).length === 0 && (
              <p className="settings-hint" data-testid="hire-no-connections">
                No account is connected yet, so there is nothing to grant.
              </p>
            )}
            {(connections.data ?? []).map((connection) => (
              <div className="hire-connection" key={connection.id}>
                <strong>{connection.display_name}</strong>
                <span>{connection.account ?? connection.alias}</span>
                <CapabilityPicker
                  selected={draft.capabilities[connection.id] ?? []}
                  onChange={(next) =>
                    patch({
                      capabilities: {
                        ...draft.capabilities,
                        [connection.id]: next,
                      },
                    })
                  }
                />
              </div>
            ))}
            <div className="hire-mailbox">
              <h5>Mailbox</h5>
              {offers.length === 0 ? (
                <NoMailboxProvider api={api} />
              ) : (
                <>
                  <label>
                    <input
                      type="checkbox"
                      aria-label="Give this sprite its own mailbox"
                      checked={draft.mailboxWanted}
                      onChange={(event) =>
                        patch({ mailboxWanted: event.target.checked })
                      }
                    />
                    Give {draft.name.trim()} its own mailbox
                  </label>
                  {draft.mailboxWanted && (
                    <MailboxFields
                      api={api}
                      offers={offers}
                      draft={draft.mailbox}
                      onChange={(next) => {
                        setNamedMailbox(true);
                        patch({ mailbox: next });
                      }}
                    />
                  )}
                </>
              )}
            </div>
          </>
        )}
        {shown !== null && (
          <p className="settings-error" role="alert">
            {shown}
          </p>
        )}
        {failure != null && (
          <p className="settings-error" role="alert">
            {errorMessage(failure, "That sprite could not be hired.")}
          </p>
        )}
      </div>
    </Dialog>
  );
}
