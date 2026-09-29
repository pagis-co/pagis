interface FrameLike {
  url: string
}

interface WebContentsLike {
  id: number
  getURL(): string
  mainFrame: FrameLike
}

interface EventLike {
  sender: WebContentsLike
  senderFrame: FrameLike | null
}

export function isTrustedSetupRequest(
  event: EventLike,
  setup: WebContentsLike,
  setupUrl: string,
): boolean {
  if (event.sender !== setup || setup.getURL() !== setupUrl) return false
  return event.senderFrame !== null
    && event.senderFrame === setup.mainFrame
    && event.senderFrame.url === setupUrl
}
