// The composer draft store: one draft per scope, taken once.

import { beforeEach, describe, expect, it } from 'vitest'

import { selectDraft, useComposerDraft } from './composerDraft'

describe('useComposerDraft', () => {
  beforeEach(() => useComposerDraft.setState({ byScope: {} }))

  it('holds a draft for the scope that asked for it', () => {
    useComposerDraft.getState().set('ch1', 'Please set up a schedule')

    expect(selectDraft('ch1')(useComposerDraft.getState())).toBe(
      'Please set up a schedule',
    )
    expect(selectDraft('ch2')(useComposerDraft.getState())).toBeUndefined()
  })

  it('clearing drops the draft and leaves the other scopes alone', () => {
    useComposerDraft.getState().set('ch1', 'one')
    useComposerDraft.getState().set('ch2', 'two')

    useComposerDraft.getState().clear('ch1')

    expect(selectDraft('ch1')(useComposerDraft.getState())).toBeUndefined()
    expect(selectDraft('ch2')(useComposerDraft.getState())).toBe('two')
  })

  it('clearing a scope with no draft changes nothing', () => {
    const before = useComposerDraft.getState().byScope

    useComposerDraft.getState().clear('ch1')

    expect(useComposerDraft.getState().byScope).toBe(before)
  })
})
