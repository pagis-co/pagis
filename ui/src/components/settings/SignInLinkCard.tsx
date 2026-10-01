// A Sign-In Link as a person hands it on (ADR-0028): the QR code that a
// phone camera opens, the address with a copy button, and the time the
// link has left. The daemon draws the QR code, so the page shows an SVG
// it received and makes none of its own.

import { useEffect, useState } from 'react'

import type { SignInLinkDto } from '../../api/client'
import { Button, Input } from '../../primitives'

import './SignInLinkCard.css'

const SECOND = 1_000
const MINUTE = 60 * SECOND
const HOUR = 60 * MINUTE
const DAY = 24 * HOUR

/** How long a link has left at `now`, in the words the card shows. Days
 *  and hours are rounded, so an invite made now has 7 days; the last hour
 *  counts down to the second. */
export function timeLeft(expiresAt: number, now: number): string {
  const left = expiresAt - now
  if (left <= 0) return 'This link expired. Make a new one.'
  if (left >= DAY) {
    const days = Math.round(left / DAY)
    return `Expires in ${days} ${days === 1 ? 'day' : 'days'}.`
  }
  if (left >= HOUR) {
    const hours = Math.round(left / HOUR)
    return `Expires in ${hours} ${hours === 1 ? 'hour' : 'hours'}.`
  }
  const minutes = Math.floor(left / MINUTE)
  const seconds = Math.floor((left % MINUTE) / SECOND)
  return `Expires in ${minutes}:${String(seconds).padStart(2, '0')}.`
}

export function SignInLinkCard({
  link,
  now = Date.now,
}: {
  link: SignInLinkDto
  /** The clock the time left reads. Tests pass a fixed one. */
  now?: () => number
}) {
  const [at, setAt] = useState(now)
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    const timer = window.setInterval(() => setAt(now()), SECOND)
    return () => window.clearInterval(timer)
  }, [now])
  const expired = at >= link.expires_at

  const copy = () => {
    // A browser that refuses the clipboard leaves the address in the
    // field, where the person selects it by hand.
    void navigator.clipboard
      ?.writeText(link.url)
      .then(() => setCopied(true))
      .catch(() => undefined)
  }

  return (
    <div className="sign-in-link">
      {!expired && (
        <img
          className="sign-in-link-qr"
          src={`data:image/svg+xml;utf8,${encodeURIComponent(link.qr_svg)}`}
          alt="QR code of the sign-in link"
        />
      )}
      <div className="sign-in-link-address">
        <Input
          readOnly
          aria-label="Sign-in link"
          value={link.url}
          onFocus={(event) => event.currentTarget.select()}
        />
        <Button size="sm" disabled={expired} onClick={copy}>
          {copied ? 'Copied' : 'Copy'}
        </Button>
      </div>
      <span className="sign-in-link-left" role="status">
        {timeLeft(link.expires_at, at)} It works once.
      </span>
    </div>
  )
}
