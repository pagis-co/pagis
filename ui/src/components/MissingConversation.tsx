// The view of a conversation address that names no conversation of
// the person. The daemon answers such an address as not found, so the
// view says that the conversation does not exist. It does not call it
// a failed load and it does not say whose it is.

import { MessageSquare } from 'lucide-react'

import { Button } from '../primitives'
import { PageState } from './PageState'

import './MissingConversation.css'

export function MissingConversation({
  onOpenHome,
}: {
  onOpenHome: () => void
}) {
  return (
    <div className="missing-conversation">
      <PageState icon={MessageSquare} title="This conversation does not exist">
        No conversation of yours has this address.
        <Button onClick={onOpenHome}>Go to Home</Button>
      </PageState>
    </div>
  )
}
