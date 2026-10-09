import { useEffect, useRef, useState } from 'react'
import { Plus, X, ArrowUp } from 'lucide-react'

import { uploadArtifact, type ApiClient } from '../api/client'
import { useSendMessage } from '../queries'
import { selectDraft, useComposerDraft } from '../state/composerDraft'
import { useSpeaking } from '../state/stores'
import { threadScope } from '../timeline'
import { IconButton, Menu, Textarea, TooltipButton } from '../primitives'
import { randomId } from '../ids'
import { Dictation } from '../ws/dictation'
import { CommandMenu, matchCommands, type Command } from './composer/CommandMenu'
import { voiceLook, voiceState } from './composer/voice'

import './Composer.css'
import { useIsMobile } from '../state/useIsMobile'

/** One uploaded attachment waiting in the composer. */
interface Attachment {
  id: string
  name: string
  mime: string
}

/** One utterance in progress: the text heard so far, shown as
 *  pending until the final transcript makes it editable. */
interface Utterance {
  pending: string
  /** Whether the daemon streams text live; unknown until `ready`. */
  live: boolean | null
  held: boolean
}

function dictationUrl(channelId: string): string {
  const scheme = window.location.protocol === 'https:' ? 'wss' : 'ws'
  return `${scheme}://${window.location.host}/api/v1/channels/${channelId}/dictate`
}

/** Append a transcript to the draft with one space between them. */
function joinDraft(draft: string, transcript: string): string {
  if (transcript === '') return draft
  if (draft === '' || /\s$/.test(draft)) return draft + transcript
  return `${draft} ${transcript}`
}

export function Composer({
  api,
  channelId,
  rootId,
  placeholder = 'Message',
  adoptsDraft = true,
}: {
  api: ApiClient
  channelId: string
  /** Set in the thread pane: sends become replies in that thread. */
  rootId?: string
  placeholder?: string
  /** A draft written for a Channel belongs to that Channel's own
   *  Thread. A composer that only reaches into the Channel from
   *  another page, as Home does (ADR-0022), leaves the draft for the
   *  Thread the reader is sent to. */
  adoptsDraft?: boolean
}) {
  const phone = useIsMobile()
  const [text, setText] = useState('')
  const [attachments, setAttachments] = useState<Attachment[]>([])
  const [uploading, setUploading] = useState(0)
  const [uploadError, setUploadError] = useState<string | null>(null)
  const [utterance, setUtterance] = useState<Utterance | null>(null)
  const [dictationError, setDictationError] = useState<string | null>(null)
  const [dragging, setDragging] = useState(false)
  const [activeCommand, setActiveCommand] = useState(0)
  const [commandsDismissed, setCommandsDismissed] = useState(false)
  const fileInput = useRef<HTMLInputElement | null>(null)
  const dictation = useRef<Dictation | null>(null)
  const send = useSendMessage(api, channelId, rootId)
  const scope = threadScope(channelId, rootId)
  const draft = useComposerDraft(selectDraft(adoptsDraft ? scope : ''))
  const clearDraft = useComposerDraft((state) => state.clear)
  const enableSpeaking = useSpeaking((state) => state.enable)

  // A surface that asked an Agent for something wrote the first
  // message. The composer takes it once, and the user edits it.
  useEffect(() => {
    if (!adoptsDraft || draft === undefined) return
    setText((current) => (current === '' ? draft : current))
    clearDraft(scope)
  }, [adoptsDraft, draft, scope, clearDraft])

  // Leaving the composer drops an utterance in progress.
  useEffect(
    () => () => {
      dictation.current?.cancel()
      dictation.current = null
    },
    [],
  )

  // Each file uploads immediately; send waits for the uploads.
  const addFiles = (files: Iterable<File>) => {
    setUploadError(null)
    for (const file of files) {
      setUploading((n) => n + 1)
      uploadArtifact(file)
        .then((dto) =>
          setAttachments((list) =>
            list.some((a) => a.id === dto.id)
              ? list
              : [...list, { id: dto.id, name: dto.filename ?? file.name, mime: dto.mime }],
          ),
        )
        .catch((error: unknown) =>
          setUploadError(error instanceof Error ? error.message : 'upload failed'),
        )
        .finally(() => setUploading((n) => n - 1))
    }
  }

  const pickFiles = () => fileInput.current?.click()

  // Push-to-talk (ADR-0020): hold to speak, release to commit.
  // Holding also turns speaking on for this scope: a user who speaks
  // wants to listen.
  const hold = () => {
    if (dictation.current !== null) return
    setDictationError(null)
    enableSpeaking(scope)
    setUtterance({ pending: '', live: null, held: true })
    const session = new Dictation({
      url: dictationUrl(channelId),
      handlers: {
        onReady: (live) =>
          setUtterance((current) => (current === null ? current : { ...current, live })),
        onDelta: (delta) =>
          setUtterance((current) =>
            current === null ? current : { ...current, pending: current.pending + delta },
          ),
        onFinal: (transcript) => {
          dictation.current = null
          setUtterance(null)
          setText((draft) => joinDraft(draft, transcript))
        },
        onError: (message) => {
          dictation.current = null
          setUtterance(null)
          setDictationError(message)
        },
      },
    })
    dictation.current = session
    session.start()
  }

  const release = () => {
    if (dictation.current === null) return
    setUtterance((current) => (current === null ? current : { ...current, held: false }))
    dictation.current.release()
  }

  const dictating = utterance !== null
  const voice = voiceState(utterance)
  const look = voiceLook(voice)
  const canSubmit =
    !dictating && uploading === 0 && (text.trim() !== '' || attachments.length > 0)

  const submit = () => {
    if (!canSubmit) return
    send.mutate({
      pendingId: randomId(),
      text: text.trim(),
      artifactIds: attachments.map((a) => a.id),
    })
    setText('')
    setAttachments([])
  }

  // The `/` menu holds the same actions as the controls of the row, so
  // the keyboard reaches every one of them. "Talk" opens the
  // microphone; the voice control ends the utterance.
  const commands: Command[] = [
    {
      id: 'attach',
      label: 'Attach files',
      description: 'Pick files to send with the message',
      run: pickFiles,
    },
    {
      id: 'talk',
      label: 'Talk',
      description: 'Dictate the message; the voice control ends it',
      run: hold,
    },
  ]
  const commanding = !dictating && !commandsDismissed && text.startsWith('/')
  const matched = commanding ? matchCommands(commands, text.slice(1)) : []
  const commandOpen = matched.length > 0

  const runCommand = (command: Command) => {
    setText('')
    setActiveCommand(0)
    command.run()
  }

  const changeText = (next: string) => {
    setText(next)
    setActiveCommand(0)
    if (!next.startsWith('/')) setCommandsDismissed(false)
  }

  const shownText = utterance === null ? text : joinDraft(text, utterance.pending)
  const listening =
    utterance !== null && utterance.held
      ? utterance.live === false
        ? 'Listening…'
        : null
      : utterance !== null
        ? 'Transcribing…'
        : null

  return (
    <form
      className={`composer${dragging ? ' composer-dragging' : ''}`}
      onSubmit={(event) => {
        event.preventDefault()
        submit()
      }}
      onDragOver={(event) => {
        event.preventDefault()
        setDragging(true)
      }}
      onDragLeave={(event) => {
        // Moving between children fires a leave on the parent too.
        if (event.currentTarget.contains(event.relatedTarget as Node | null)) return
        setDragging(false)
      }}
      onDrop={(event) => {
        event.preventDefault()
        setDragging(false)
        const files = Array.from(event.dataTransfer?.files ?? [])
        if (files.length > 0) addFiles(files)
      }}
    >
      {dragging && (
        <div className="composer-dropzone" data-testid="composer-dropzone">
          Drop files to attach them
        </div>
      )}
      {(attachments.length > 0 ||
        uploading > 0 ||
        uploadError !== null ||
        dictationError !== null) && (
        <div className="composer-attachments" data-testid="composer-attachments">
          {attachments.map((attachment) => (
            <span className="composer-attachment" key={attachment.id}>
              {attachment.name}
              <IconButton
                icon={X}
                label={`Remove ${attachment.name}`}
                variant="ghost"
                size="sm"
                className="composer-attachment-remove"
                onClick={() =>
                  setAttachments((list) =>
                    list.filter((a) => a.id !== attachment.id),
                  )
                }
              />
            </span>
          ))}
          {uploading > 0 && (
            <span className="composer-uploading">Uploading…</span>
          )}
          {uploadError !== null && (
            <span className="composer-upload-error">{uploadError}</span>
          )}
          {dictationError !== null && (
            <span className="composer-upload-error" data-testid="dictation-error">
              {dictationError}
            </span>
          )}
        </div>
      )}
      {commandOpen && (
        <CommandMenu
          commands={matched}
          activeIndex={activeCommand}
          onRun={runCommand}
        />
      )}
      <div className="composer-row">
        <input
          ref={fileInput}
          type="file"
          multiple
          hidden
          data-testid="composer-file-input"
          onChange={(event) => {
            if (event.target.files !== null) addFiles(event.target.files)
            event.target.value = ''
          }}
        />
        <Menu
          label="Attach"
          trigger={
            <IconButton
              icon={Plus}
              label={phone ? "Add a file" : "Attach"}
              variant="ghost"
            />
          }
          items={[{ label: 'Attach files', onSelect: pickFiles }]}
        />
        <Textarea
          bare={!phone}
          pill={phone}
          className={`composer-input${dictating ? ' composer-input-dictating' : ''}`}
          value={shownText}
          placeholder={listening ?? placeholder}
          rows={1}
          readOnly={dictating}
          aria-busy={dictating}
          aria-expanded={commandOpen}
          onChange={(event) => changeText(event.target.value)}
          onPaste={(event) => {
            const files = Array.from(event.clipboardData.files)
            if (files.length > 0) {
              event.preventDefault()
              addFiles(files)
            }
          }}
          onKeyDown={(event) => {
            // Typing while the button is held ends the dictation and
            // keeps what was transcribed (ADR-0020).
            if (dictating) {
              release()
              return
            }
            if (commandOpen) {
              if (event.key === 'ArrowDown') {
                event.preventDefault()
                setActiveCommand((index) => (index + 1) % matched.length)
                return
              }
              if (event.key === 'ArrowUp') {
                event.preventDefault()
                setActiveCommand(
                  (index) => (index - 1 + matched.length) % matched.length,
                )
                return
              }
              if (event.key === 'Escape') {
                event.preventDefault()
                setCommandsDismissed(true)
                return
              }
              if (event.key === 'Enter' && !event.shiftKey) {
                event.preventDefault()
                runCommand(matched[activeCommand] ?? matched[0])
                return
              }
            }
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault()
              submit()
            }
          }}
        />
        {phone && canSubmit ? <IconButton icon={ArrowUp} label="Send" variant="primary" shape="pill" type="submit" /> : <IconButton
          icon={look.icon}
          label={phone && voice === 'idle' ? 'Dictate a message' : look.label}
          variant={phone ? 'primary' : 'ghost'}
          shape={phone ? 'pill' : undefined}
          className={`composer-mic${voice === 'listening' ? ' composer-mic-held' : ''}`}
          data-voice-state={voice}
          data-testid="composer-mic"
          aria-pressed={voice === 'listening'}
          onPointerDown={(event) => {
            event.preventDefault()
            hold()
          }}
          onPointerUp={release}
          onPointerLeave={release}
          onPointerCancel={release}
          onKeyDown={(event) => {
            if (event.key !== ' ' && event.key !== 'Enter') return
            event.preventDefault()
            if (!event.repeat) hold()
          }}
          onKeyUp={(event) => {
            if (event.key !== ' ' && event.key !== 'Enter') return
            event.preventDefault()
            release()
          }}
          onBlur={() => {
            if (utterance?.held) release()
          }}
          onContextMenu={(event) => event.preventDefault()}
        />}
        {!phone && <TooltipButton
          variant="primary"
          type="submit"
          disabled={!canSubmit}
          tooltip="Send the message (Enter)"
        >
          Send
        </TooltipButton>}
      </div>
    </form>
  )
}
