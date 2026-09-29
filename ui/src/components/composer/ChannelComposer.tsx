// The composer of one Channel, or the reason the Channel has none
// (ADR-0003). A Channel the Agents opened between themselves is
// read-only for the user: a message from the user in it would start an
// Agent-to-Agent exchange the user has no part in. A Channel the user
// made with several Agents stays writable, because the user is in it.

import type { ApiClient } from '../../api/client'
import { useChannels } from '../../queries'
import { Composer } from '../Composer'

import './ChannelComposer.css'

export function ChannelComposer({
  api,
  channelId,
  rootId,
  placeholder,
  adoptsDraft,
}: {
  api: ApiClient
  channelId: string
  /** Set in the thread pane: sends become replies in that thread. */
  rootId?: string
  placeholder?: string
  /** See `Composer`: a composer that reaches into a Channel from
   *  another page leaves that Channel's draft alone. */
  adoptsDraft?: boolean
}) {
  const channels = useChannels(api)
  const channel = channels.data?.find((row) => row.id === channelId)

  if (channel !== undefined && !channel.user_member) {
    return (
      <p className="composer-read-only" data-testid="composer-read-only">
        The sprites talk to each other here. You can read this conversation.
      </p>
    )
  }
  return (
    <Composer
      api={api}
      channelId={channelId}
      rootId={rootId}
      placeholder={placeholder}
      adoptsDraft={adoptsDraft}
    />
  )
}
