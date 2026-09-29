import { describe, expect, it } from 'vitest'

import { quitQuestion } from './quitQuestion'

// Quit from the setup page, the app menu or the tray asks a question only
// while an installation or a start-up is in progress. Else it quits at
// once, as a desktop app does.
describe('the question that Quit asks', () => {
  it('asks nothing when nothing is in progress', () => {
    expect(quitQuestion(false)).toBeNull()
  })

  it('asks before it stops an installation or a start-up, and Cancel is the default', () => {
    const question = quitQuestion(true)

    expect(question).toEqual({
      type: 'question',
      message: 'Quit Pagis?',
      detail: 'The installation or the start-up in progress stops.',
      buttons: ['Quit', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })
})
