# The router's modalities

`crates/llm-router` carries text, tools, images, audio, video and
realtime voice over one neutral interface. This document states the
shape of each one and what is absent on purpose. The router's own
design is in `docs/DESIGN.md`.

## Four rules

1. **The OpenAI shapes are the neutral schema** for every request and
   response type. The field converged there for images, video and audio,
   so the neutral shape costs the least translation.
2. **Long-running generation is an async job** with a router-scoped id:
   create it, read its status, fetch its content. Video generation uses
   it.
3. **An absent capability is declared, never emulated.** A provider that
   cannot do something returns `Error::Unsupported` or omits the
   capability flag. The caller then reads the absence and decides.
4. **Realtime is a separate subsystem** with its own module and its own
   dependencies. The HTTP `Protocol` trait does not grow socket methods.

## Audio in chat

A content part carries input audio as bytes with its format. A request
names its modalities and an output voice and format, and the assistant's
audio comes back as a content part with an optional transcript and
expiry. The stream has its own events for an audio delta and an audio
transcript delta, apart from a text delta, so an interface renders them
apart. A multi-turn replay passes the audio back by id.

## Speech and transcription

Two endpoints with their own wire patterns, bytes out and multipart in:

```text
SpeechRequest { model, input, voice, format?, speed?, instructions?, extra }
  -> SpeechResponse { audio: Bytes, media_type }

TranscriptionRequest { model, audio: Bytes, media_type, language?, prompt?,
                       timestamps, extra }
  -> TranscriptionResponse { text, segments?, words?, language?, duration_s? }
```

The `Protocol` trait carries both as default-unsupported methods that
return a built request and a response decoder, which is the shape
embeddings and images use.

Speech returns buffered bytes, because an Agent speaks one block at a
time. Streaming speech is not built.

Per-minute and per-character pricing are optional fields on the model
record, so a cost stays absent where the rate is unknown.

## Image generation

One request carries the prompt, optional input images and a mask, a size
as either pixels or an aspect ratio and tier, and the quality, the output
format and the background. The router picks the generation or the edit
route by whether input images are present. The response carries the media
type, an optional revised prompt, and the usage and cost.

Streaming partial images is not built: one provider offers them, so the
router returns the whole image.

## Video generation

Video is the async job:

```text
VideoRequest { model, prompt, seconds?, size?, input_image?, extra }

VideoJob { id, status: Queued | InProgress | Completed | Failed | Canceled,
           progress?, created_at, expires_at?, video_url?, error? }
```

The router creates a job, reads its status, and fetches its content as
bytes, proxying a signed URL with authentication where the provider needs
it. The job id is router-scoped and encodes the provider and the native
id, so a status or a content call routes with no extra state.

The caller polls the status. A webhook is not built. Remix, extend and
edit are not built, because only one provider has them.

## Realtime voice

The router resolves a model alias, opens the upstream socket with the
credentials, and hands it over. The application pumps the frames and taps
the cheap events for metering. This works for every provider on the
OpenAI dialect, and it is the path that the dictation of a Thread takes:
a transcription-only session is the same dialect, so live speech to text
needs no translation layer. Deepgram's live speech to text is its own
dialect on `/listen`: the router opens it with the model and the audio
format in the query and names it `DeepgramListen`, and the application
reads its `Results` with an adapter of its own.

A provider whose wire events differ needs a session adapter that
translates them, with its own declared capabilities: turn detection, user
transcription, audio output, automatic tool reply, truncation and
resumption. Its events are the boundary facts alone — ready, audio delta,
transcript, speech started and stopped, interrupted, tool call, turn
complete with usage, closed — and the adapter never emulates a missing
one. Voice-activity policy is not abstracted.

## What is absent

Streaming image generation, streaming speech, the chat-bridge modalities
of a provider that has no native codec, and the conversation session
adapters for the providers that are not on the OpenAI dialect. Each is
absent because no consumer needs it, and each is declared absent rather
than emulated.
