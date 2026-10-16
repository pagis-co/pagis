# 0020: Pagis is a SIP endpoint and a texting client of one Org carrier

Status: accepted.

## Context

A telephone number must call and text on every installation, including a
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
it from storage, no message id the counterpart echoes, and a Workspace Number
belongs to a Workspace while an Agent Mailbox belongs to an Agent (ADR-0041).
A United States number also needs
registration (local) or verification (toll-free) before carriers deliver its
texts; two of three carriers accept the API call and fail the message later.

## Decision

### A Workspace holds its number

ADR-0041 defines Workspace Number ownership, inbound Call routing, Call
hand-off and inbound text delivery. The Person sets the number in Settings.
Outbound Calls need a Workspace Number Grant (ADR-0041).
Not built: outbound texts from the Workspace Number.

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
  purchase and again at daemon start where it is missing, and adds the
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
bought. The Person manages the Workspace Number in Settings on the product
port (ADR-0018).

The carrier's line resolves the dialed E.164 to the active Workspace Number.
The Workspace's Chief of Staff answers. The line refuses before answer when
the number is absent, ambiguous, released or has no active Chief of Staff
(ADR-0041).

### The media plane

- One line for each carrier Connection owns the registration and the socket.
It starts when the Connection has a SIP credential and restarts when the
credential changes; number purchases do not change it. It refreshes at half
the expiry and backs off to a 30-second cap. Its state is visible, because an
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
- The RTP uplink has a reorder window of about 40 ms and no playout buffer.
  A frame leg passes inbound audio frames in order with no reorder window
  or RTP state. It carries G.711 µ-law, G.711 A-law, or signed little-endian
  PCM16 at 8 kHz. `FrameLeg::pair` gives an Embedder bounded channels
  for inbound events and answer frames. This leg accepts typed inbound
  keypad events and declares outbound digits absent. A frame leg without
  typed keypad events declares digits absent in both directions.
  The SIP endpoint uses a packet leg. No carrier media stream adapter is
  built.
- Both legs use a 20 ms downlink pacer and stay 60 ms deep or less. Audio
  converts at the model and recorder boundaries when their codecs differ.
  Tasks and timers come from `pagis-rt`.

The call core of `pagis-telephony` builds on
`wasm32-unknown-unknown` with its default features. Audio conversion, DTMF,
the Media Hub, the Keypad Code check, the Realtime Bridge, Call sessions,
Call tools and hand-off use the same code on both targets. `CallEndpoints`
supplies the bridge's carrier operations. A native installation supplies
its SIP endpoints. An Embedder supplies its own carrier operations and
model transport.

SIP signaling, SIP media, the endpoint manager, the socket listener, the
carrier REST clients and disk recording stay native. The frame and packet
Media Leg types contain no native runtime calls. An Embedder can subscribe
to the Media Hub's recording stream without a disk recorder. The Call
record routes and live control of `pagis-server` are portable. The socket
upgrade stays native (ADR-0038).

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

### The Run stays for the Call

An inbound Call opens a Run for the Chief of Staff. The realtime session is
the model loop while the Call runs. A hand-off opens a Run for the target
Agent, changes the Call holder and starts its model session on the same
Media Hub. The earlier Run ends. The new Run uses its own voice, Standing
Brief and free tools. Only an active Agent of the same Workspace can take
the Call (ADR-0041).

A live Call uses only free tools. An action that needs approval waits until
the Call ends. The Trust Tier narrows the tools (ADR-0021). The session tools
hang up, send keypad digits and hand off the Call. Hand-off needs a confirmed
Trust Tier. Outbound Calls need a Workspace Number Grant (ADR-0041).

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
transcript_file_id?, recording_file_id?
created_at, answered_at?, ended_at?
```

The outcome and the ended reason answer two questions: a call can reach its
purpose and end with a media timeout. The transcript and recording are
Files with a class; each class has a retention window, none by default, and
a daily sweep deletes expired Files. The audit log gets placed, answered,
ended and tier-changed events with the number, the Agent and the Run, and no
credential.

The tool result carries the outcome, the ended reason, the classification,
whether a message was left, the duration, the transcript File id and the
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
outcome, the ended reason, the duration and the transcript File, never the
words. Its filter takes callers, a minimum trust and answered or not.

The Chief of Staff's Standing Call Rule wakes the Agent after a Call. The
daemon reads the designation at Call time and uses the Agent's own Standing
Brief. A hand-off changes the Call holder and the Agent that reads the
settled Call. `call_transcript` reads the holder's Call and returns the words
inside the untrusted envelope with their Trust Tier (ADR-0041).

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
realtime socket. Deepgram streams on its own `/listen` socket: the daemon
sends the PCM frames as they come, each final result adds to the draft, and
the release sends `CloseStream`, after which Deepgram sends its last results
and closes. ElevenLabs streams on Scribe's realtime socket with a manual
commit: the daemon sends each PCM frame as a base64 chunk, the release
sends a commit, and the committed transcript is the final draft.

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
each speech model in its model list, Deepgram lists each Aura voice as a
model, ElevenLabs lists the voices of the account, and OpenAI lists no
voices, so its fixed set is the list. A voice has an id, which the Agent
holds, and a name where the id is not one, which a person reads. The daemon
validates a new voice against the list. The voice
is the voice of spoken replies and of Calls. Where the model that speaks lacks
it, because the `speak` alias changed provider, the reply takes the model's
first voice and says which voice spoke. A Call runs on OpenAI, so a voice that
is not an OpenAI voice is absent there: the Call uses the OpenAI default, and
the record says which voice was used. A provider that fixes the voice once a
session has emitted audio takes a change at the next Call.

### Texting

The text collector polls each active Workspace Number. It stores a Text
Record for the Workspace Number and wakes the current Chief of Staff through
a Standing Text Rule. The record holds the counterpart, direction, Trust
Tier, body, segment count, carrier id and time. The sender's number supplies
the tier without a Keypad Code. The words reach the Run inside the untrusted
envelope. A stored cursor and event identity prevent duplicate delivery.
The `text.message_received` Incoming Event carries the number, sender,
Trust Tier, receipt time, segment count, media flag and Text Record id.
It contains no body. The Run reads the body from the Text Record store.

Not built: outbound texts from the Workspace Number,
text tools, Messaging Readiness reads and display, the text Outgoing Cap,
MMS media storage and the Conversation Inspector. The carrier transport and
Text Record types contain the fields for these capabilities.

## Consequences

- The number belongs to the Workspace; the Call names its current Agent.
- A restart ends a live Call with `daemon_restart`.
- A slow media subscriber does not stop the carrier audio.
- A provider with no realtime socket still supports Dictation.
- The carrier key stays out of the model session.
