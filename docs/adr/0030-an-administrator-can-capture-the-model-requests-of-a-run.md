# 0030: An Administrator can capture the model requests of a Run

Status: accepted.

## Context

A Run records a summary of each model request: the route, the estimated
input tokens, the output limit, and the number of messages, tools and
images (`model.requested`). It records the provider's error as text. It
does not record the content of a request. When a provider refuses a
request, the Administrator cannot see what Pagis sent.

A request holds the system prompt, the conversation after Compaction, the
tool results and the images. On a Server, it can hold data that belongs to
other People. Each Turn sends the whole context again, so a Run of twenty
Turns can hold megabytes.

Products that route model requests make content capture a choice, and keep
the metadata of each request without it. OpenRouter stores prompts and
completions only after the account turns on input and output logging.
LiteLLM stores the request in its spend logs only when
`store_prompts_in_spend_logs` is on. Langfuse and Helicone store the full
trace, and give masking and retention settings.

## Decision

### A System Setting, off by default

**Model Request Capture** is a System Setting. It is off by default. An
Administrator turns it on in the Administration Interface and sets its
retention: 7 days by default, from 1 to 30 days. The daemon writes both in
the `[model_request_capture]` table of `config.toml`, and the change applies
with no restart. The setting is for the whole installation, and not for one
Agent or one Workspace.

While it is on, the Product App tells each Person that Pagis keeps their
model requests, and for how many days.

### What a capture holds

The daemon takes one capture for each model request of a Run, beside the
`model.completed` event of the same request:

- **The request**, as the Agent sends it after Compaction: the Model Alias
  and its candidates, the system prompt, the messages with their tool calls
  and tool results, the tool definitions, the output limit and the output
  schema. Each image is replaced by its media type, its size in bytes, its
  size in pixels and its SHA-256. The request is not the body on the wire:
  the router adds the tool of the Computer for each candidate and puts the
  request into the form of each provider.
- **The answer**: the outcome, the serving provider and model, the stop
  reason and the usage of a request that completed; the HTTP status, the
  message and the provider's error body of a request that failed.

A capture holds no secret. The model never sees a Credential secret
(ADR-0013), and the provider key goes in a header that the capture does not
hold.

### Only an Administrator reads a capture

The run page shows **Show request** on each model request that has a
capture, to an Administrator only. The route of a capture answers `404` to
a Member, as it does for a Run that does not exist.

### A capture expires

- The daily retention sweep deletes each capture older than its retention.
- When an Administrator turns the setting off, the daemon deletes every
  capture.
- Forget deletes the captures of each Run that read the forgotten source,
  in the same transaction that removes the tool results of that Run
  (ADR-0008).
- A Backup leaves the captures out. On SQLite, the backup deletes them from
  its copy of the database and compacts the copy. On Postgres, `pg_dump`
  skips the rows of the table.

## Consequences

- The provider's HTTP status and error body now travel with a model
  failure from the router to the Run.
- A capture is a copy of Person data. While the setting is on, the database
  holds that copy for the retention of the setting.
- The capture of the request is in the form of the Agent, so it does not
  show a fault that only the provider form of a request has.
