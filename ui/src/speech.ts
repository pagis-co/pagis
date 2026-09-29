// Spoken replies (ADR-0020): when a Thread speaks, every
// `markdown` block of a settled Agent message is synthesized on its
// own and played in order. Every other block type is silent, and no
// caption is spoken for it. Messages queue, so two replies never talk
// over each other. Nothing is kept: a replay fetches the audio again.

import { fetchSpeech } from './api/client'

/** The blocks of one message, as the timeline carries them. */
export interface SpokenMessage {
  blocks: readonly { type: string }[]
}

export interface SpeakerDeps {
  fetchMessage: (channelId: string, messageId: string) => Promise<SpokenMessage>
  fetchSpeech: (channelId: string, messageId: string, block: number) => Promise<Blob>
  /** Play one clip to its end. */
  play: (audio: Blob) => Promise<void>
}

/** The indexes of the blocks a message speaks: its `markdown` blocks. */
export function spokenBlocks(message: SpokenMessage): number[] {
  const indexes: number[] = []
  message.blocks.forEach((block, index) => {
    if (block.type === 'markdown') indexes.push(index)
  })
  return indexes
}

export class Speaker {
  private queue: Promise<void> = Promise.resolve()

  constructor(private readonly deps: SpeakerDeps) {}

  /** Speak one message after the ones already queued. Resolves when
   *  it has been played; a failed block is skipped, not retried. */
  speak(channelId: string, messageId: string): Promise<void> {
    const turn = this.queue.then(() => this.speakNow(channelId, messageId))
    this.queue = turn.catch(() => undefined)
    return turn
  }

  private async speakNow(channelId: string, messageId: string): Promise<void> {
    const message = await this.deps.fetchMessage(channelId, messageId)
    for (const block of spokenBlocks(message)) {
      const audio = await this.deps.fetchSpeech(channelId, messageId, block)
      await this.deps.play(audio)
    }
  }
}

/** Play a clip through an `<audio>` element and resolve when it ends. */
export function playClip(audio: Blob): Promise<void> {
  return new Promise((resolve) => {
    const url = URL.createObjectURL(audio)
    const element = new Audio(url)
    const done = () => {
      URL.revokeObjectURL(url)
      resolve()
    }
    element.onended = done
    element.onerror = done
    void element.play().catch(done)
  })
}

/** The one speaker of the app, over the daemon's REST surface. */
export function createSpeaker(
  fetchMessage: SpeakerDeps['fetchMessage'],
): Speaker {
  return new Speaker({ fetchMessage, fetchSpeech, play: playClip })
}
