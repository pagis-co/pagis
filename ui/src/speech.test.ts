// Spoken replies (ADR-0020): only markdown blocks are spoken, in
// order, one synthesis per block; messages queue.

import { describe, expect, it } from 'vitest'

import { Speaker, spokenBlocks } from './speech'

function deps(messages: Record<string, { type: string }[]>) {
  const log: string[] = []
  let release: (() => void) | null = null
  const speaker = new Speaker({
    fetchMessage: async (_channelId, messageId) => ({ blocks: messages[messageId] }),
    fetchSpeech: async (_channelId, messageId, block) => {
      log.push(`fetch:${messageId}:${block}`)
      if (block === 99) throw new Error('unavailable')
      return new Blob([`${messageId}:${block}`])
    },
    play: (audio) =>
      new Promise((resolve) => {
        void audio.text().then((text) => {
          log.push(`play:${text}`)
          release = resolve
        })
      }),
  })
  return { speaker, log, release: () => release?.() }
}

async function flush() {
  for (let i = 0; i < 5; i += 1) await Promise.resolve()
}

describe('spokenBlocks', () => {
  it('names the markdown blocks and nothing else', () => {
    expect(
      spokenBlocks({
        blocks: [
          { type: 'markdown' },
          { type: 'table' },
          { type: 'form' },
          { type: 'choice_card' },
          { type: 'approval_card' },
          { type: 'progress' },
          { type: 'image' },
          { type: 'file' },
          { type: 'markdown' },
        ],
      }),
    ).toEqual([0, 8])
    expect(spokenBlocks({ blocks: [{ type: 'table' }] })).toEqual([])
  })
})

describe('Speaker', () => {
  it('plays each markdown block in order and skips the silent ones', async () => {
    const d = deps({
      m1: [{ type: 'markdown' }, { type: 'table' }, { type: 'markdown' }],
    })

    const done = d.speaker.speak('ch', 'm1')
    await flush()
    expect(d.log).toEqual(['fetch:m1:0', 'play:m1:0'])

    d.release()
    await flush()
    expect(d.log).toEqual(['fetch:m1:0', 'play:m1:0', 'fetch:m1:2', 'play:m1:2'])

    d.release()
    await done
  })

  it('a message with no markdown block makes no sound', async () => {
    const d = deps({ m1: [{ type: 'table' }, { type: 'file' }] })

    await d.speaker.speak('ch', 'm1')

    expect(d.log).toEqual([])
  })

  it('queues messages so two replies never talk over each other', async () => {
    const d = deps({ m1: [{ type: 'markdown' }], m2: [{ type: 'markdown' }] })

    const first = d.speaker.speak('ch', 'm1')
    const second = d.speaker.speak('ch', 'm2')
    await flush()
    expect(d.log).toEqual(['fetch:m1:0', 'play:m1:0'])

    d.release()
    await first
    await flush()
    expect(d.log).toEqual(['fetch:m1:0', 'play:m1:0', 'fetch:m2:0', 'play:m2:0'])
    d.release()
    await second
  })

  it('a failed synthesis fails that message and the queue moves on', async () => {
    const d = deps({ m1: [{ type: 'markdown' }], m2: [{ type: 'markdown' }] })
    const failing = new Speaker({
      fetchMessage: async () => ({ blocks: [{ type: 'markdown' }] }),
      fetchSpeech: async () => {
        throw new Error('unavailable')
      },
      play: async () => undefined,
    })

    await expect(failing.speak('ch', 'm1')).rejects.toThrow('unavailable')
    // The queue is not stuck behind the failure.
    const next = d.speaker.speak('ch', 'm2')
    await flush()
    d.release()
    await next
  })
})
