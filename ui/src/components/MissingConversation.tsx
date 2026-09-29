// The view of a conversation address that names no conversation of
// the person. The daemon answers such an address as not found, so the
// view says that the conversation does not exist. It does not call it
// a failed load and it does not say whose it is.

import { Menu as MenuIcon, MessageSquare } from 'lucide-react'

import { Button, IconButton } from '../primitives'
import { PageState } from './PageState'

import './MissingConversation.css'

export function MissingConversation({
  onOpenNav,
  onOpenHome,
}: {
  onOpenNav: () => void
  onOpenHome: () => void
}) {
  return (
    <div className="missing-conversation">
      <IconButton
        icon={MenuIcon}
        label="Open conversations"
        variant="ghost"
        className="mobile-navigation-trigger"
        onClick={onOpenNav}
      />
      <PageState icon={MessageSquare} title="This conversation does not exist">
        No conversation of yours has this address.
        <Button onClick={onOpenHome}>Go to Home</Button>
      </PageState>
    </div>
  )
}
