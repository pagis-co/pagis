# 0020: Pagis is a SIP endpoint and a texting client of one Org carrier

Status: accepted.

## Context

An Agent Phone Number must call and text on every installation, including a
laptop behind NAT with no public address or tunnel. The numbers live on the
Org's own carrier account.

A credential SIP connection is the only surveyed call path that needs no
public ingress: signaling goes out as a register, and the carrier corrects the
RTP address and port when they differ from the offer. Every provider media
WebSocket is dialed by the provider, so none works behind NAT. A realtime model
API accepts G.711 in each direction, so carrier bytes reach the model
unchanged, with no transcode.

Texting is REST in both directions, and no carrier offers a channel that a
daemon behind NAT can dial. A text differs from mail: there is no host to fetch
it from later, no message id the counterpart echoes, and a number moves between
Agents where a mailbox never does. A United States number also needs
registration (local) or verification (toll-free) before carriers deliver its
texts; two of three carriers accept the API call and fail the message later.

## Decision

### Pagis terminates SIP and RTP itself

The daemon is a SIP user agent. It registers outward, sends and answers
invites, sends a bye, and holds the media socket for the call. It uses no
provider call-control API and no provider media stream. So recording is Pagis's
own, answering-machine detection (a call-control feature) is declared absent,
the media path is G.711 at 8 kHz from carrier to model and back, and the router
opens the provider endpoint while the telephony crate owns the typed
call-session adapter and the event translation.

### Three seams

Telephony has three duties with different credentials and failure modes, so it
has three traits in `crates/pagis-telephony`, each with a fake and a
configurable capability set beside it:

- `NumberCatalog`: search, buy, find and release a number, and prepare the
  carrier's SIP connection, with the REST account key.
- `CallTransport`: signaling and media, with the SIP credential. It carries its
  declared capability set. The account key stays out of the call path.
- `TextTransport`: `capabilities()` (texting, inbound media);
  `prepare(number)`, which puts the carrier's messaging object in place at
  assignment and again at daemon start where it is missing, and adds the
  account-level ids to the Org carrier Connection's config for reuse;
  `send(from, to, body) -> carrier id`, which sends the whole body up to the
  carrier's limit and reports the segment count, and maps a country the
  account has not enabled to `destination_not_enabled`;
  `delivery_status(id)`; `poll_inbound(number, cursor) -> (texts, cursor)`;
  and `fetch_media(url) -> bytes`. It uses the account key.

Every carrier accepts an outward register from a credential, so the SIP
transport is one provider-neutral implementation whose registrar comes from
the SIP credential. The number and text seams have one implementation for each
carrier id, picked by the carrier Connection's provider; Telnyx and Twilio have
both. A carrier with no text implementation declares texting absent, and its
catalog entry says so before a number is bought. A carrier key is an account id
in the Connection config and a secret stored as an installation secret whose
name carries the provider and no Workspace.

### One carrier serves the Org

The installation holds one carrier Connection, an Installation Connection in
the Org's Workspace (ADR-0023), which carries every number of every person. An
Administrator sets up the carrier and its SIP credential as the `connection`
and `sip` parts of its catalog entry (ADR-0024). Where a provider's call path
needs something the installation lacks, the entry says so before a number is
bought. A person manages their own numbers on the product port (ADR-0018).

The carrier's line routes an inbound call by its dialed number, normalized to
E.164, to the stored Agent Phone Number, which names the Workspace and Agent.
It refuses a call before answering when the dialed number is missing, not held
by the installation, held by no Agent, or matches more than one record.

### The media plane

- One line for each carrier Connection owns the registration and the socket.
  It starts when the Connection has a SIP credential and restarts when the
  credential changes; assignments do not touch it. It refreshes at half the
  expiry and backs off to a 30-second cap. Its state is visible, because an
  unregistered line drops inbound calls silently.
- One active call for each number; a second inbound call gets busy.
- One media hub for each call owns both directions and fans them out to the
  model uplink, the recorder and listen-live. Subscribers are lossy, so a slow
  browser cannot stall an RTP write. Typed events (ringing, answered, keypad
  tone, ended) travel with the audio in order.
- The offer carries G.711 only, 20 ms packets and RTCP multiplexing, over TLS
  signaling with SRTP media. There is no cleartext path.
- The daemon sends RTP for the whole call, filling silence, to keep the NAT
  binding and the RTP latch alive.
- The uplink has a reorder window of about 40 ms and no playout buffer. The
  downlink has a 20 ms pacer and stays 60 ms deep or less.

Where the provider emits a speech-start event, it is the only voice activity
detection: the bridge clears the pending downlink and truncates the model's
item at the audio actually sent. The shallow queue makes this exact. A
full-duplex provider emits no such event and takes no truncation, and its
adapter invents neither; the hub still keeps a shallow queue and can drop
queued output on an interruption signal of the application.

### The model session is the provider boundary

`ModelSession` takes typed commands (configuration, audio, tool results,
context, response continuation, truncation) and gives typed events (audio,
transcript, interruption, tool, usage, error). Each provider has its own
adapter. Workspace aliases select the conversation model, the reasoning model
of a delegating live protocol, and the audio-capable answer detector. A
protocol change needs an adapter, not a new alias value.

### The Run stays for the call

The Run that places a call stays alive for the call. The realtime session is
the model loop while the call runs, and afterwards the same Run reads the
result and does any follow-up. A park and wake would park a Run whose model
loop runs. Tool calls from the session go through the broker, and the bridge
receives the trust tier (ADR-0021) and binds the session to it.

```text
phone_call {
    to                  E.164, required
    brief               what the call is for, required
    success_criteria    how the Agent judges the result, required
    voicemail           leave a message or hang up, default hang up
    max_duration_s      per-call override of the workspace cap
}
```

The Agent writes what the call is for. The daemon writes who calls and under
which limits: the identity and voice, the tier, the duration cap, the
phone-tree prompt, the classify phase and the emergency rule (ADR-0018).
Hang-up and send-digits are session tools, bound only inside a live call. The
call tool is `Outbound`, and there is no allow-rule builder for a number, so
every call asks.

A live call is a free-effect zone. The session gets only `Free` tools, because
a remote party cannot wait while the user decides; an action that needs an
approval happens after the call, in the same Run. The session gets the tools
the brief names, intersected with the `Free` tools of the Run's snapshot, and
the tier then applies.

### The classify phase

On an outbound call a realtime session starts before dialing and listens only
after the carrier reports an answer, because ringback skews the verdict.

| Verdict | What the call does | Outcome |
| --- | --- | --- |
| `human` | the conversation starts | `answered` |
| `machine-ivr` | phone-tree mode starts | what the conversation reaches |
| `machine-vm` | leave the message, or hang up | `voicemail` |
| `machine-unavailable` | hang up | `failed` |
| `uncertain` | the conversation starts | `answered` |

`machine-unavailable` is `failed`, because `no_answer` means "try again the
same way". `uncertain` continues, because ten seconds with a machine costs less
than cutting off a person. Phone-tree mode starts from the verdict, because the
Agent rarely knows beforehand.

### The Call record

```text
id, workspace_id, agent_id, run_id, phone_number_id
direction               outbound | inbound
remote_e164
tier                    owner | trusted | unknown
state                   dialing | live | ended
outcome                 answered | no_answer | busy | voicemail | failed
ended_reason, classification, message_left
transcript_artifact_id?, recording_artifact_id?
created_at, answered_at?, ended_at?
```

The outcome and the ended reason answer two questions: a call can reach its
purpose and end with a media timeout. The transcript and recording are
Artifacts with a class; each class has a retention window, none by default, and
a daily sweep deletes expired Artifacts. The audit log gets placed, answered,
ended and tier-changed events with the number, the Agent and the Run, and no
credential.

The tool result carries the outcome, the ended reason, the classification,
whether a message was left, the duration, the transcript Artifact id and the
transcript itself inside the untrusted envelope with its tier, so the Agent
can judge its success criteria. The recording is a pointer.

Every call ends with a reason. A dropped model socket reconnects once with the
transcript, else hangs up with `model_unavailable`. Ten seconds of no inbound
RTP and no bye ends with `media_timeout`. Cleartext media or a failed encrypted
session ends at once with `media_failed`. A daemon restart ends the call: the
record settles from disk with `daemon_restart`, and the recording is muxed from
the appended files. The duration cap (a workspace setting, ten minutes by
default) counts from the answer: a wrap-up instruction 30 seconds before, then
a bye. An outbound call whose model session does not open is not placed.

### An inbound call wakes the Agent afterwards

A settled call raises a `call.ended` Incoming Event, declared once for each
carrier provider, from the inbound runner, with no collector. Its metadata is
the Call id (its identity), the direction, the line, the caller, the tier, the
outcome, the ended reason, the duration and the transcript Artifact, never the
words. Its filter takes callers, a minimum trust and answered or not.

A Standing Call Rule is written when a number becomes an Agent's: every call,
the Agent's own Channel with the user, the user's provenance, an instruction to
read the call. Taking the line back archives it. A `Free` core tool reads one
of the Agent's own Calls and returns the words inside the untrusted envelope
with their tier; another Agent's Call reads as absent.

Listen-live is a WebSocket on the daemon's authenticated server that sends a
mono mix of both directions as G.711, which the browser decodes. A listener
hears from the moment of joining. Media fan-out and control are separate.

### Push-to-talk, and one voice for each Agent

The user holds a button in the composer, speaks and releases; release ends the
utterance, so there is no voice activity detection and no wake word. The
browser opens a dictate socket for the Channel and sends PCM; the daemon relays
it to a transcription-only provider session with manual commit and returns
deltas and a final transcript. Audio stays off the domain-event socket, and the
provider credential stays in the daemon. Where live transcription is absent,
the daemon transcribes the held clip on release. OpenRouter is such a
provider: it transcribes a clip on `/audio/transcriptions` and has no
realtime socket.

The transcript is a draft in the composer, never a sent message, because
speech-to-text mishears names and numbers and the message goes to an Agent
with Grants.

Spoken replies are one toggle for each Thread, off by default, and holding the
microphone turns it on for that Thread. Only `markdown` blocks are spoken; the
daemon writes no caption for another block. It synthesizes one block at a time
and the browser plays them in order. No spoken audio is stored: a clip is
discarded after transcription and speech is streamed. A Call's recording is the
exception, because there the recording is the record.

Two workspace aliases carry the transcription and speech models. The Agent
record carries one voice from the Provider Voice List: the voices of the model
that speaks, which is the first candidate of the `speak` alias whose provider
holds a key and serves spoken replies. The daemon reads the list from that
provider, as it reads the Provider Model List: OpenRouter names the voices of
each speech model in its model list, and OpenAI lists no voices, so its fixed
set is the list. The daemon validates a new voice against the list. The voice
is the voice of spoken replies and of Calls. Where the model that speaks lacks
it, because the `speak` alias changed provider, the reply takes the model's
first voice and says which voice spoke. A Call runs on OpenAI, so a voice that
is not an OpenAI voice is absent there: the Call uses the OpenAI default, and
the record says which voice was used. A provider that fixes the voice once a
session has emitted audio takes a change at the next Call.

### Texting

Messaging Readiness is a state of an Agent Phone Number, read from the
carrier: `unknown`, `not_capable`, `unregistered`, `pending`, `ready` or
`rejected(reason)`. The number record keeps the state, its reason, the read
time and the last failed read's error. Readiness never gates an inbound text,
because carriers filter outbound only. Pagis detects and shows readiness and
does not register, because registration collects a tax id, an address, an
opt-in sample and a fee, and waits days for human review.

Pagis stores each text as a Text Record: the Agent, the number, the
counterpart, the direction, the tier, the body, the segment count, the media
pointers, the carrier message id and the time; an outbound record adds the
Run, the Thread and the delivery status (`queued`, `sent`, `delivered`, or
`failed` with the carrier code and reason). The outbound record is the reply
lookup, so there is no sent-texts table. The number record holds the text
Outgoing Cap (50 a day by default), the send card's allow rules, the daily
tally and the inbound cursor. A Text Conversation is the pair of one number
and one counterpart, derived and never stored. Records stay with their Agent,
so a new holder of a number sees none of the previous holder's texts.

## Consequences

- An Agent on a call cannot take an approved action until the call ends.
- A brief that names a tool the Agent lacks gets a smaller tool set and no
  error.
- Every call asks for an approval.
- A daemon restart during a call ends it with `daemon_restart`, and the Run
  fails with every other unfinished Run.
- A provider with no realtime socket still dictates; the text arrives at the
  end.
- A reply that is only a table is silent in a spoken Thread.
- A carrier key rotation is one write, and a number is unique in the
  installation.
- Texting needs one table, and the Trust List's numbers serve calls and texts.

## Not built

The text seam, the readiness type, the Text Record and the number fields
exist. No path sends, receives or shows a text. The design of that path:

- **Collector.** One task for each assigned number whose transport declares
  texting polls every ten seconds, backs off to five minutes on a rate limit
  or error, and saves the cursor on the number after each batch. The cursor
  starts at assignment. The carrier joins inbound segments. Media is fetched at
  ingest with the account key and stored as an Artifact before the event.
  Deduplication on the Connection, the kind and the carrier message id.
- **Relay.** Where a carrier has no listable inbound store, the daemon installs
  a small first-party relay in the carrier account: it downloads the pinned CLI
  release, verifies its checksum, writes the configuration with the key-value
  namespace and the webhook public key, and ships a handler that is Pagis
  source, versioned with the daemon. The handler verifies the webhook signature
  before it writes, and the daemon polls the namespace. The relay has its own
  state on the number page (absent, installing, ready, failed with a reason)
  and gates inbound collection alone.
- **Readiness read.** A number catalog method reads readiness with the account
  key. A poller reads at assignment, hourly while not ready and daily when
  ready. The number page shows the state with a refresh control and a link to
  the carrier's registration console with a checklist. A send from a number
  that is not ready is refused with a tool error naming the state and reason,
  after one fresh read where the state is unknown or older than a day.
- **Delivery polling** at 5 seconds, 30 seconds, 2 minutes and 10 minutes;
  a text with no receipt stays `sent`.
- **Where an inbound text lands.** In the Thread of the last outbound record of
  the pair that carries a Thread and is under seven days old, else in the
  Agent's own Channel with the user.
- **Event and Standing Text Rule.** `text.message_received` carries the
  number, the sender, the tier, the time, the segment count, whether there is
  media and the text id, never a body; its filter takes senders, a minimum
  trust and replies only. A burst joins one Wake-up, and a text during a live
  call from the same number is a Wake-up, not a call event. The sender's number
  alone stamps the tier, with no keypad code. Assigning a number creates the
  Standing Text Rule on the Agent (the text kind, the Agent's number, an
  instruction to read the conversation, the Agent's own Channel, the user's
  provenance, revision one); the user pauses, edits or deletes it, and the
  Agent may narrow it and never delete it. Unassigning deletes the rule.
- **Three core tools**, acting as the Agent's own identity with no Grant:
  `text_send(to, body)` returns the id, a queued status, the segment count and
  the cap left; `text_conversation(with, max <= 50, before)` returns the
  records with one counterpart, bodies inside the untrusted envelope, media as
  Artifact pointers; `text_conversations(max <= 25)` returns recent
  counterparts. An Agent with no number has no text tools; with one, the tools
  exist whatever the readiness, and a send that is not ready answers
  `number_not_messaging_ready`. Reads are `Free`, send is `Outbound`; the card
  shows the number, the recipient and the body, and offers one allow rule, the
  counterpart number, stored on the number record. The Trust List gives no
  exemption. A send over the cap fails with `outgoing_cap_reached` before any
  card, counting messages, not segments. A send to an emergency number gets
  the call's refusal. A call brief may name the reads; a send never runs inside
  a call.
- **A failed delivery** raises `text.delivery_failed`, which wakes the Agent
  in the Thread the text was sent from.
- **The `text` block and the Conversation Inspector.** A daemon-made `text`
  block for each record that woke the Agent or that it sent: the direction,
  the counterpart with its Trust List name, the tier chip, the Agent's number,
  and the body inline, folded past about 160 characters; an outbound block
  shows the delivery state and a failure reason. It opens the Text Conversation
  in the inspector: the counterpart and tier, the Agent and number, readiness
  where not ready, records by day with delivery states and media, and a link
  from each text to its Thread. The inspector has no compose box: the user
  tells the Agent to text.
