import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { workspacePath } from './workspace-path'

describe('Workspace request paths', () => {
  it('matches the role extensions in the API contract', () => {
    const contract = JSON.parse(readFileSync('openapi.json', 'utf8'))
    for (const [path, item] of Object.entries(contract.paths)) {
      for (const [method, operation] of Object.entries(item as Record<string, Record<string, unknown>>)) {
        if (!['get', 'post', 'put', 'patch', 'delete', 'head', 'options', 'trace'].includes(method)) continue
        const expected = operation['x-workspace-role']
          ? `/api/v1/workspaces/team${path.slice('/api/v1'.length)}`
          : path
        expect(workspacePath(path, 'team', method), `${method} ${path}`).toBe(expected)
      }
    }
  })

  it('uses the prefix only for Shared Workspace content', () => {
    expect(workspacePath('/api/v1/channels?before=1', 'team')).toBe('/api/v1/workspaces/team/channels?before=1')
    expect(workspacePath('/api/v1/workspace', null)).toBe('/api/v1/workspace')
    for (const path of ['/api/v1/settings/sessions', '/api/v1/settings/sign-in-links', '/api/v1/push/key', '/api/v1/user', '/api/v1/sessions', '/api/v1/conversations', '/api/v1/colleagues', '/api/v1/workspaces']) {
      expect(workspacePath(path, 'team')).toBe(path)
    }
  })
})
