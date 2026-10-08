// What the Client App says about an Update (ADR-0027).

import { describe, expect, it } from 'vitest'

import { checkAnswer, readyNotification, restartQuestion } from './updateMessages'

describe('the answer of Check for Updates', () => {
  it('says that the client is up to date, with its release', () => {
    expect(checkAnswer({ kind: 'up-to-date' }, '1.0.0')).toMatchObject({
      type: 'info',
      message: 'Pagis is up to date.',
      detail: 'Pagis 1.0.0 is the newest release.',
    })
  })

  /** A connected Client App takes no Update past the release of its
   *  server (ADR-0027). */
  it('says that a connected client is up to date with its server, with both releases', () => {
    expect(checkAnswer({ kind: 'up-to-date-with-server', server: '1.0.0' }, '1.0.0')).toMatchObject({
      type: 'info',
      message: 'Pagis is up to date with its server.',
      detail: 'This app is Pagis 1.0.0, and its server runs Pagis 1.0.0. ' +
        'A connected Pagis takes only the Update to the release of its server.',
    })
  })

  it('names the Update that downloads and what installs it', () => {
    expect(checkAnswer({ kind: 'found', version: '1.1.0' }, '1.0.0', 'darwin')).toMatchObject({
      type: 'info',
      message: 'Pagis 1.1.0 is available.',
      detail: 'Pagis downloads it now. When the download is complete, select Restart to Update in the Pagis menu.',
    })
    expect(checkAnswer({ kind: 'ready', version: '1.1.0' }, '1.0.0', 'darwin')).toMatchObject({
      message: 'Pagis 1.1.0 is ready to install.',
      detail: 'Select Restart to Update in the Pagis menu.',
    })
  })

  /** On Linux the Update item is in the tray menu and in the File menu of
   *  the window. */
  it('names the menus of the Update item on Linux', () => {
    expect(checkAnswer({ kind: 'found', version: '1.1.0' }, '1.0.0', 'linux').detail).toBe(
      'Pagis downloads it now. When the download is complete, select Restart to Update in the tray menu or in the File menu.',
    )
    expect(checkAnswer({ kind: 'ready', version: '1.1.0' }, '1.0.0', 'linux').detail).toBe(
      'Select Restart to Update in the tray menu or in the File menu.',
    )
    expect(readyNotification('1.1.0', 'linux').body).toBe('Select Restart to Update in the tray menu or in the File menu.')
  })

  it('gives the reason of a failed check', () => {
    expect(checkAnswer({ kind: 'failed', reason: 'net::ERR_INTERNET_DISCONNECTED' }, '1.0.0')).toMatchObject({
      type: 'error',
      message: 'Pagis could not check for updates.',
      detail: 'net::ERR_INTERNET_DISCONNECTED',
    })
  })
})

describe('the question that Restart to Update asks', () => {
  it('asks nothing when no Run is in progress and no Coding Session runs', () => {
    expect(restartQuestion(0, 0)).toBeNull()
  })

  it('says how many Runs a restart stops, and Cancel is the default', () => {
    expect(restartQuestion(3, 0)).toEqual({
      type: 'question',
      message: 'Restart Pagis to install the Update?',
      detail: '3 Runs have not finished. A restart stops them.',
      buttons: ['Restart', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
    expect(restartQuestion(1, 0)?.detail).toBe('1 Run has not finished. A restart stops it.')
  })

  it('says how many Coding Sessions on this computer a restart stops, and Cancel is the default', () => {
    expect(restartQuestion(0, 2)).toEqual({
      type: 'question',
      message: 'Restart Pagis to install the Update?',
      detail: '2 Coding Sessions run on this computer. A restart stops them.',
      buttons: ['Restart', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })

  it('names the Runs and the Coding Sessions that a restart stops', () => {
    expect(restartQuestion(1, 1)).toEqual({
      type: 'question',
      message: 'Restart Pagis to install the Update?',
      detail: '1 Run has not finished. A restart stops it. ' +
        '1 Coding Session runs on this computer. A restart stops it.',
      buttons: ['Restart', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    })
  })

  it('asks with a general message when the server does not give the count', () => {
    expect(restartQuestion(null, 0)).toMatchObject({
      message: 'Restart Pagis to install the Update?',
      detail: 'Pagis cannot count the Runs in progress. A restart stops each Run that has not finished.',
      buttons: ['Restart', 'Cancel'],
    })
  })
})

describe('the notification of a ready Update', () => {
  it('names the release and what installs it', () => {
    expect(readyNotification('1.1.0', 'darwin')).toEqual({
      title: 'Pagis 1.1.0 is ready to install',
      body: 'Select Restart to Update in the Pagis menu.',
    })
  })
})
