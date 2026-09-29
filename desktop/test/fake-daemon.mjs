#!/usr/bin/env node
// A stand-in for the Pagis daemon, so that the shell's lifecycle tests
// need no built binary. It answers the health endpoint, honours SIGINT,
// exits with the restart code, prints the taken-port message when the
// port is held, and can hold a port itself. As the real daemon does, it
// writes a Client Credential and answers the runtime identity only when
// it starts with `--local`. When the config file names an Administration
// Port, it binds that port as well, after the product port, and prints the
// daemon's own message when that port is held. The hold mode holds the
// product port alone.
//
// Environment:
//   PAGIS_HOME    the data directory, as the real daemon reads it
//   FAKE_VERSION  the version the health endpoint reports
//   FAKE_MODE     serve (default) | crash | hold
//   FAKE_EXIT_CODE  the exit code of the crash mode
//
// It writes the PAGIS_SUPERVISED it got to `fake-supervised` in its
// home, so a test reads what the shell told it.

import * as crypto from 'node:crypto'
import * as fs from 'node:fs'
import * as http from 'node:http'
import * as path from 'node:path'

const home = process.env.PAGIS_HOME
if (!home) {
  console.error('PAGIS_HOME is not set')
  process.exit(2)
}
const version = process.env.FAKE_VERSION ?? '0.1.0'
const image = process.env.FAKE_IMAGE ?? `ghcr.io/pagis-co/pagis-computer@sha256:${'b'.repeat(64)}`
const mode = process.env.FAKE_MODE ?? 'serve'
fs.writeFileSync(path.join(home, 'fake-supervised'), process.env.PAGIS_SUPERVISED ?? '')

if (mode === 'crash') {
  console.error('fake daemon: the boot failed')
  process.exit(Number(process.env.FAKE_EXIT_CODE ?? 1))
}

const port = readPort()
const administrationPort = mode === 'hold' ? null : readAdministrationPort()
const credential = process.argv.includes('--local') ? writeClientCredential() : null

const server = http.createServer((request, response) => {
  if (mode === 'hold') {
    response.writeHead(404).end('not pagis')
    return
  }
  if (request.method === 'GET' && request.url === '/api/v1/health') {
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ status: 'ok', version }))
    return
  }
  if (request.method === 'GET' && request.url?.startsWith('/api/v1/runtime/identity?')) {
    if (credential === null) {
      response.writeHead(404).end()
      return
    }
    const challenge = new URL(request.url, `http://127.0.0.1:${port}`).searchParams.get('challenge')
    if (!challenge || !/^[0-9a-f]{64}$/.test(challenge)) {
      response.writeHead(422).end()
      return
    }
    const workspaceId = 'fake-workspace'
    const message = ['pagis-runtime-identity-v1', String(port), version, workspaceId, image, challenge].join('\0')
    const proof = crypto.createHmac('sha256', credential).update(message).digest('hex')
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(JSON.stringify({
      status: 'ok',
      release: version,
      workspace_id: workspaceId,
      port,
      computer_image: image,
      proof,
    }))
    return
  }
  if (request.method === 'POST' && request.url === '/api/v1/system/restart') {
    response.writeHead(200, { 'content-type': 'application/json' })
    response.end(JSON.stringify({ exit_code: 75 }), () => {
      server.close()
      process.exit(75)
    })
    return
  }
  response.writeHead(404).end()
})

server.on('error', (error) => {
  if (error.code === 'EADDRINUSE') {
    // The daemon's own message, word for word (ADR-0025).
    console.error(
      `port ${port} is already in use. Stop the process that holds it, or start ` +
        'Pagis on another port with `pagis --port <PORT>`.',
    )
    process.exit(1)
  }
  console.error(String(error))
  process.exit(1)
})

server.listen(port, '127.0.0.1', () => {
  if (administrationPort === null) {
    console.log(`open http://127.0.0.1:${port}/`)
    return
  }
  const administration = http.createServer((_request, response) => response.writeHead(404).end())
  administration.on('error', (error) => {
    if (error.code === 'EADDRINUSE') {
      // The daemon's own message, word for word.
      console.error(
        `Error: the administration port ${administrationPort} is already in use. Stop the ` +
          'process that holds it, or name another port in `[administration] port` of config.toml.',
      )
      process.exit(1)
    }
    console.error(String(error))
    process.exit(1)
  })
  administration.listen(administrationPort, '127.0.0.1', () => {
    console.log(`open http://127.0.0.1:${port}/`)
  })
})

for (const signal of ['SIGINT', 'SIGTERM']) {
  process.on(signal, () => {
    server.close()
    process.exit(0)
  })
}

function readPort() {
  try {
    const text = fs.readFileSync(path.join(home, 'config.toml'), 'utf8')
    const found = /^\s*port\s*=\s*(\d+)/m.exec(text.split('[')[0])
    return found ? Number(found[1]) : 4400
  } catch {
    return 4400
  }
}

function readAdministrationPort() {
  try {
    const text = fs.readFileSync(path.join(home, 'config.toml'), 'utf8')
    const table = /^\s*\[administration\]\s*$([^[]*)/m.exec(text)
    const found = table && /^\s*port\s*=\s*(\d+)/m.exec(table[1])
    return found ? Number(found[1]) : null
  } catch {
    return null
  }
}

function writeClientCredential() {
  const file = path.join(home, 'client-credential')
  if (!fs.existsSync(file)) {
    fs.mkdirSync(home, { recursive: true })
    fs.writeFileSync(file, 'a'.repeat(64), { mode: 0o600 })
  }
  return fs.readFileSync(file, 'utf8').trim()
}

