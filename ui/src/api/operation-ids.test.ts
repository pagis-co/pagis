import { readFileSync } from 'node:fs'
import { expect, it } from 'vitest'

it('gives each API operation a distinct identifier', () => {
  const contract = JSON.parse(readFileSync('openapi.json', 'utf8'))
  const ids = Object.values(contract.paths as Record<string, Record<string, { operationId?: string }>>)
    .flatMap((path) => Object.values(path).flatMap((operation) => operation.operationId ? [operation.operationId] : []))
  expect(new Set(ids).size).toBe(ids.length)
})
