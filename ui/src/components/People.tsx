import type { ApiClient } from '../api/client'
import { ColleaguePicker } from './people/ColleaguePicker'
import { ConversationList } from './people/ConversationList'

import './People.css'

export function People({
  api,
  onOpen,
}: {
  api: ApiClient
  onOpen: (id: string) => void
}) {
  return (
    <section className="people-page">
      <h1>People</h1>
      <ColleaguePicker api={api} onOpen={onOpen} />
      <ConversationList api={api} onOpen={onOpen} />
    </section>
  )
}
