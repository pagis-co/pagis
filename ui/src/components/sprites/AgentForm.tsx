// The Agent profile form: name, job, description, personality
// and voice. It is the one form behind "Edit" on the profile.
//
// Creation does not come through here: hiring is four steps of its own
// (`HireAgent.tsx`), and it carries the Mailbox section (ADR-0019).

import { useState } from "react";

import type { ApiClient } from "../../api/client";
import { Button, Input, Select } from "../../primitives";
import { errorMessage, useVoices } from "../../queries";

import "../agent.css";
import "../settings.css";

/** What "leave the voice to the provider" is worth inside the Select: a
 *  Radix item cannot carry an empty value, and the form writes `null`. */
const PROVIDER_DEFAULT_VOICE = "provider-default";

/** The profile fields the form writes. `voice` is the Agent Voice
 *  (ADR-0020); `null` leaves it to the provider. */
export interface AgentFields {
  name: string;
  job: string;
  /** One line on what to ask this agent for. The other agents read it. */
  description: string;
  personality: string;
  voice: string | null;
}

export function AgentForm({
  api,
  initial,
  submitLabel,
  pending,
  failure = null,
  onSubmit,
  onCancel,
}: {
  api: ApiClient;
  initial: AgentFields;
  submitLabel: string;
  pending: boolean;
  failure?: unknown;
  onSubmit: (fields: AgentFields) => void;
  onCancel?: () => void;
}) {
  const [name, setName] = useState(initial.name);
  const [job, setJob] = useState(initial.job);
  const [description, setDescription] = useState(initial.description);
  const [personality, setPersonality] = useState(initial.personality);
  const [voice, setVoice] = useState(initial.voice ?? "");
  const voices = useVoices(api);

  return (
    <form
      className="agent-form"
      onSubmit={(e) => {
        e.preventDefault();
        onSubmit({
          name,
          job,
          description,
          personality,
          voice: voice === "" ? null : voice,
        });
      }}
    >
      <Input
        aria-label="Sprite name"
        placeholder="Name"
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      <Input
        aria-label="Sprite job"
        placeholder="Job, e.g. research assistant"
        value={job}
        onChange={(e) => setJob(e.target.value)}
      />
      <Input
        aria-label="Sprite description"
        placeholder="What to ask this sprite for"
        value={description}
        onChange={(e) => setDescription(e.target.value)}
      />
      <Input
        aria-label="Sprite personality"
        placeholder="Personality"
        value={personality}
        onChange={(e) => setPersonality(e.target.value)}
      />
      <Select
        label="Sprite voice"
        value={voice === "" ? PROVIDER_DEFAULT_VOICE : voice}
        onValueChange={(next) =>
          setVoice(next === PROVIDER_DEFAULT_VOICE ? "" : next)
        }
        items={[
          { value: PROVIDER_DEFAULT_VOICE, label: "Provider default voice" },
          ...(voices.data ?? []).map((name) => ({ value: name, label: name })),
        ]}
      />
      {failure != null && (
        <p className="settings-error" role="alert">
          {errorMessage(failure, "That sprite could not be saved.")}
        </p>
      )}
      <div className="agent-form-actions">
        <Button
          type="submit"
          variant="primary"
          disabled={pending || name.trim() === ""}
        >
          {submitLabel}
        </Button>
        {onCancel !== undefined && (
          <Button type="button" onClick={onCancel}>
            Cancel
          </Button>
        )}
      </div>
    </form>
  );
}
