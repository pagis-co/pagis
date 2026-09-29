// Trusted contacts (ADR-0021, ADR-0022, ADR-0019). The section
// follows the settings grammar: a title line with a one-sentence lead,
// the keypad code first, then phone numbers and email senders as framed
// tables with an inline add row, and the tier explainer as the hint.
//
// The rows are Workspace state that decides the authority of a caller
// or a sender to any Agent. The addresses of the user's own mail
// Connections are owner without a row (ADR-0019): they show in the
// senders table with no Remove.
//
// The shell around this section (title bar, settings nav) is not here:
// the settings grammar wraps it.

import { useState } from "react";

import type { ApiClient } from "../api/client";
import type { components } from "../api/schema";
import {
  Badge,
  Button,
  Frame,
  Input,
  Row,
  SectionLabel,
  Select,
} from "../primitives";
import type { BadgeTone } from "../primitives";
import {
  errorMessage,
  useAddTrustEntry,
  useClearKeypadFailures,
  useDeleteTrustEntry,
  useDeleteKeypadCode,
  useSetKeypadCode,
  useTrustList,
} from "../queries";
import { formatMoment } from "../timeline";
import { formatE164 } from "./AgentPhoneNumber";

import "./TrustedContacts.css";
import "./settings.css";

type TrustEntry = components["schemas"]["TrustEntryDto"];
type OwnAddress = components["schemas"]["OwnAddressDto"];
type KeypadCode = components["schemas"]["KeypadCodeDto"];

const NO_KEYPAD_CODE: KeypadCode = {
  configured: false,
  failed_attempts: 0,
  suspended_until: null,
};

const TIER_TONE: Record<string, BadgeTone> = {
  owner: "accent",
  trusted: "working",
};

const TIER_NAME: Record<string, string> = {
  owner: "Owner",
  trusted: "Trusted",
};

function TierChip({ tier }: { tier: string }) {
  return (
    <Badge tone={TIER_TONE[tier] ?? "neutral"}>
      {TIER_NAME[tier] ?? "Unknown"}
    </Badge>
  );
}

/** The two tables. A number decides a Call, an address or a domain
 *  decides mail. */
type Table = {
  id: "numbers" | "senders";
  title: string;
  noun: string;
  subjects: string[];
  valuePlaceholder: string;
  labelPlaceholder: string;
};

const TABLES: Table[] = [
  {
    id: "numbers",
    title: "Phone numbers",
    noun: "number",
    subjects: ["number"],
    valuePlaceholder: "+1 …",
    labelPlaceholder: "Label, e.g. Home",
  },
  {
    id: "senders",
    title: "Email senders",
    noun: "sender",
    subjects: ["address", "domain"],
    valuePlaceholder: "address or domain",
    labelPlaceholder: "Label",
  },
];

/** How one entry reads: a number in its dialing form, an address as it
 *  was stored, a domain with the at sign that says it covers addresses. */
function entryText(row: TrustEntry) {
  if (row.subject === "number") return formatE164(row.value);
  if (row.subject === "domain") return `@${row.value}`;
  return row.value;
}

function ContactRow({ api, row }: { api: ApiClient; row: TrustEntry }) {
  const remove = useDeleteTrustEntry(api);
  return (
    <Row className="trust-row" data-testid={`trust-row-${row.id}`}>
      <span className="trust-row-label">
        {row.label === "" ? "No label" : row.label}
      </span>
      <span className="trust-row-value">{entryText(row)}</span>
      <TierChip tier={row.tier} />
      {row.subject === "domain" && (
        <span className="trust-row-note">
          a domain covers every address at it
        </span>
      )}
      <span className="trust-row-spacer" />
      <Button
        variant="ghost"
        size="sm"
        aria-label={`Remove ${row.value}`}
        disabled={remove.isPending}
        onClick={() => remove.mutate(row.id)}
      >
        Remove
      </Button>
    </Row>
  );
}

/** One address that is owner because the user holds the account. It is
 *  not a row, so it carries no Remove. */
function OwnAddressRow({ row }: { row: OwnAddress }) {
  return (
    <Row className="trust-row" data-testid={`own-address-${row.address}`}>
      <span className="trust-row-label">Me</span>
      <span className="trust-row-value">{row.address}</span>
      <TierChip tier="owner" />
      <span className="trust-row-note">{`Your ${row.connection_alias} account`}</span>
    </Row>
  );
}

function AddRow({ api, table }: { api: ApiClient; table: Table }) {
  const add = useAddTrustEntry(api);
  const [label, setLabel] = useState("");
  const [value, setValue] = useState("");
  const [tier, setTier] = useState("trusted");
  return (
    <Row className="trust-row trust-add-row">
      <Input
        aria-label={`Label for the new ${table.noun}`}
        placeholder={table.labelPlaceholder}
        value={label}
        onChange={(event) => setLabel(event.target.value)}
      />
      <Input
        className="trust-row-value"
        aria-label={`New ${table.noun}`}
        placeholder={table.valuePlaceholder}
        value={value}
        onChange={(event) => setValue(event.target.value)}
      />
      <Select
        label={`Tier for the new ${table.noun}`}
        value={tier}
        onValueChange={setTier}
        items={[
          { value: "trusted", label: "Trusted" },
          { value: "owner", label: "Owner" },
        ]}
      />
      {add.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(add.error, `That ${table.noun} could not be listed.`)}
        </span>
      )}
      <span className="trust-row-spacer" />
      <Button
        size="sm"
        disabled={add.isPending || value.trim() === ""}
        onClick={() =>
          add.mutate(
            { value: value.trim(), tier, label: label.trim() },
            {
              onSuccess: () => {
                setLabel("");
                setValue("");
              },
            },
          )
        }
      >
        Add
      </Button>
    </Row>
  );
}

/** What the wrong codes of the callers came to: how many, and the end
 *  of the delay when one has started (ADR-0021). Empty with no wrong
 *  code. */
function failuresText(keypad: KeypadCode, now: number): string {
  const count = keypad.failed_attempts;
  if (count === 0) return "";
  const times = count === 1 ? "once" : `${count} times`;
  const until = keypad.suspended_until;
  if (until === null || until === undefined)
    return `Callers entered a wrong code ${times}.`;
  return now < until
    ? `Callers entered a wrong code ${times}. Pagis checks no code until ${formatMoment(until, now)}.`
    : `Callers entered a wrong code ${times}. The last delay ended at ${formatMoment(until, now)}.`;
}

/** The one code of the workspace. It is stored as a hash: there is no
 *  reveal and no export, so the field is always empty. The row also
 *  holds the wrong codes of the callers, which only the person can
 *  clear. */
function KeypadCodeRow({
  api,
  keypad,
}: {
  api: ApiClient;
  keypad: KeypadCode;
}) {
  const save = useSetKeypadCode(api);
  const remove = useDeleteKeypadCode(api);
  const clearFailures = useClearKeypadFailures(api);
  const [code, setCode] = useState("");
  const configured = keypad.configured;
  const failures = failuresText(keypad, Date.now());
  return (
    <Row className="trust-row" data-testid="keypad-code">
      <span className="trust-row-note">
        On a call Pagis answers, the number proposes a tier and this code
        confirms it.{" "}
        {configured
          ? "A code is set."
          : "No code is set, so no caller can rise above Unknown."}
        {failures !== "" && ` ${failures}`}
      </span>
      {clearFailures.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(clearFailures.error, "The count could not be cleared.")}
        </span>
      )}
      {save.isError && (
        <span className="settings-error" role="alert">
          {errorMessage(save.error, "That code could not be saved.")}
        </span>
      )}
      <span className="trust-row-spacer" />
      <Input
        className="trust-keypad-input"
        aria-label="Keypad code"
        type="password"
        inputMode="numeric"
        placeholder="6 to 8 digits"
        value={code}
        onChange={(event) => setCode(event.target.value)}
      />
      <Button
        size="sm"
        disabled={save.isPending || code.trim() === ""}
        onClick={() =>
          save.mutate(code.trim(), { onSuccess: () => setCode("") })
        }
      >
        Save
      </Button>
      {configured && (
        <Button
          variant="ghost"
          size="sm"
          aria-label="Delete the keypad code"
          disabled={remove.isPending}
          onClick={() => remove.mutate()}
        >
          Delete
        </Button>
      )}
      {keypad.failed_attempts > 0 && (
        <Button
          variant="ghost"
          size="sm"
          aria-label="Clear the failed attempts"
          disabled={clearFailures.isPending}
          onClick={() => clearFailures.mutate()}
        >
          Clear
        </Button>
      )}
    </Row>
  );
}

const TIER_EXPLAINER = (
  <span data-testid="tier-explainer">
    <strong>Owner</strong> speaks as you. <strong>Trusted</strong> is believed
    but cannot approve. <strong>Unknown</strong> is read as foreign text. Tiers
    apply everywhere in the workspace.
  </span>
);

export function TrustedContacts({ api }: { api: ApiClient }) {
  const list = useTrustList(api);
  const rows = list.data?.items ?? [];
  const ownAddresses = list.data?.own_addresses ?? [];
  return (
    <div className="trusted-contacts">
      <div className="trusted-contacts-title">
        <h3>Trusted contacts</h3>
        <span>
          What the words of a caller or a sender are worth. Never whether Pagis
          answers.
        </span>
      </div>

      <SectionLabel data-testid="section-label">Keypad code</SectionLabel>
      <Frame>
        <KeypadCodeRow
          api={api}
          keypad={list.data?.keypad_code ?? NO_KEYPAD_CODE}
        />
      </Frame>

      {TABLES.map((table, index) => {
        const inTable = rows.filter((row) =>
          table.subjects.includes(row.subject),
        );
        const fixed = table.id === "senders" ? ownAddresses : [];
        const last = index === TABLES.length - 1;
        return (
          <div key={table.id} className="trusted-contacts-table">
            <SectionLabel data-testid="section-label">
              {table.title}
            </SectionLabel>
            <Frame
              data-testid={`trust-list-${table.id}`}
              hint={last ? TIER_EXPLAINER : undefined}
            >
              {fixed.map((row) => (
                <OwnAddressRow key={row.address} row={row} />
              ))}
              {inTable.map((row) => (
                <ContactRow key={row.id} api={api} row={row} />
              ))}
              <AddRow api={api} table={table} />
            </Frame>
          </div>
        );
      })}
    </div>
  );
}
