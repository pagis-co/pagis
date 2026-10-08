import { describe, expect, it } from 'vitest'

import { quitQuestion } from './quitQuestion'

// Quit from the setup page, the app menu or the tray asks a question only
// while it stops work: an installation or a start-up in progress, or the
// Coding Sessions that run on this computer. Else it quits at once, as a
// desktop app does.
describe('the question that Quit asks', () => {
  it('asks nothing when nothing is in progress and no Coding Session runs', () => {
    expect(quitQuestion(false, 0)).toBeNull()
  })

  it('asks before it stops an installation or a start-up, and Cancel is the default', () => {
    expect(quitQuestion(true, 0)).toEqual({
      type: 'question',
      message: 'Quit Pagis?',
      detail: 'The installation or the start-up in progress stops.',
      buttons: ['Quit', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })

  it('asks before it stops the one Coding Session on this computer, and Cancel is the default', () => {
    expect(quitQuestion(false, 1)).toEqual({
      type: 'question',
      message: 'Quit Pagis?',
      detail: '1 Coding Session runs on this computer. Quitting stops it.',
      buttons: ['Quit', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })

  it('says how many Coding Sessions Quit stops, and Cancel is the default', () => {
    expect(quitQuestion(false, 3)).toEqual({
      type: 'question',
      message: 'Quit Pagis?',
      detail: '3 Coding Sessions run on this computer. Quitting stops them.',
      buttons: ['Quit', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })

  it('names each kind of work that stops, and Cancel is the default', () => {
    expect(quitQuestion(true, 2)).toEqual({
      type: 'question',
      message: 'Quit Pagis?',
      detail: 'The installation or the start-up in progress stops. ' +
        '2 Coding Sessions run on this computer. Quitting stops them.',
      buttons: ['Quit', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })
})
