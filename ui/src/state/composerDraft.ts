// The composer draft. A surface that asks an Agent for
// something writes the first message here and sends the reader to the
// DM; the composer of that scope picks the draft up once and clears
// it, so the text is editable and never comes back.

import { create } from 'zustand'

export interface ComposerDraftState {
  /** The draft of each scope (`threadScope`), until a composer takes it. */
  byScope: Record<string, string>
  /** Put the first message of a scope in the composer. */
  set: (scope: string, text: string) => void
  /** The composer took the draft: it belongs to the textarea now. */
  clear: (scope: string) => void
}

export function selectDraft(scope: string) {
  return (state: ComposerDraftState): string | undefined => state.byScope[scope]
}

export const useComposerDraft = create<ComposerDraftState>((set) => ({
  byScope: {},
  set: (scope, text) =>
    set((state) => ({ byScope: { ...state.byScope, [scope]: text } })),
  clear: (scope) =>
    set((state) => {
      if (!(scope in state.byScope)) return state
      const next = { ...state.byScope }
      delete next[scope]
      return { byScope: next }
    }),
}))
