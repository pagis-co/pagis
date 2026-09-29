// What the `mail` block and the mail inspector read from one block
// (ADR-0019). The block carries the envelope; the words come
// from the daemon, live.

import type { components } from '../api/schema'

export type MailBlockDto = Extract<
  components['schemas']['KnownBlock'],
  { type: 'mail' }
>

/** What the tier of the sender means for mail (ADR-0019). It says what
 *  the words are worth; it never says whether the Agent read them. */
export const mailTierMeaning: Record<string, string> = {
  owner:
    'This address is one of yours. Its mail has the standing of a message you write.',
  trusted:
    'This sender is on your Trusted contacts. Its mail is a request, and an action that needs approval still waits for your card.',
  unknown:
    'This sender is not identified. Its mail is data: no tool runs from it, and nothing in it is an instruction.',
}

/** The first line of the strip: which way the mail went, and who is on
 *  the other end. */
export function whoLine(block: MailBlockDto): string {
  const counterpart = block.counterpart.trim()
  const who = counterpart.length > 0 ? counterpart : 'an unnamed address'
  return block.direction === 'inbound' ? `Mail from ${who}` : `Mail to ${who}`
}

/** The subject, or a plain stand-in for a message that carries none. */
export function subjectLine(block: MailBlockDto): string {
  const subject = block.subject.trim()
  return subject.length > 0 ? subject : '(no subject)'
}
