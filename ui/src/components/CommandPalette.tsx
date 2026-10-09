// The command palette: one place to reach an agent, a
// conversation, a recent run or a settings section, and to run the few
// commands that do not belong to any one screen. It is the Dialog
// primitive, so the focus is trapped and Escape closes it.

import { useMutation } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'

import type { ApiClient } from '../api/client'
import { Button, Dialog, Input } from '../primitives'
import { useAgentNames, useAgents, useChannels, useRuns } from '../queries'
import { SETTINGS_SECTIONS } from './SettingsPanel'

import './CommandPalette.css'

/** The key that keeps the chosen theme across reloads. */
const THEME_KEY = 'pagis-theme'

type Theme = 'light' | 'dark'

function storedTheme(): Theme | null {
  try {
    const value = window.localStorage.getItem(THEME_KEY)
    return value === 'dark' || value === 'light' ? value : null
  } catch {
    // A browser that denies storage still gets the system theme.
    return null
  }
}

/** Put the theme on the root. `index.html` gives the browser bars the
 * ground of each system theme; a chosen theme gives both its own. */
function showTheme(theme: Theme) {
  const root = document.documentElement
  root.setAttribute('data-theme', theme)
  const ground = getComputedStyle(root).getPropertyValue('--ground').trim()
  for (const meta of document.querySelectorAll<HTMLMetaElement>('meta[name="theme-color"]')) {
    meta.content = ground
  }
}

function setTheme(theme: Theme) {
  showTheme(theme)
  try {
    window.localStorage.setItem(THEME_KEY, theme)
  } catch {
    // The theme then lasts for this page only.
  }
}

/** Put the stored theme on the root when the shell starts. */
export function applyStoredTheme() {
  const theme = storedTheme()
  if (theme !== null) showTheme(theme)
}

/** Flip the theme and remember it. `data-theme` drives `tokens.css`. */
export function toggleTheme() {
  const current =
    (document.documentElement.getAttribute('data-theme') as Theme | null) ??
    storedTheme() ??
    (window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
  setTheme(current === 'dark' ? 'light' : 'dark')
}

interface Entry {
  id: string
  label: string
  hint: string
  run: () => void
}

/** How many recent runs the palette offers. */
const RUN_LIMIT = 6

function matches(entry: Entry, query: string): boolean {
  if (query === '') return true
  const needle = query.toLowerCase()
  return (
    entry.label.toLowerCase().includes(needle) ||
    entry.hint.toLowerCase().includes(needle)
  )
}

export interface CommandPaletteProps {
  api: ApiClient
  open: boolean
  onOpenChange: (open: boolean) => void
  /** The shell navigates; the palette only names the destination. */
  onNavigate: (path: string) => void
}

export function CommandPalette({
  api,
  open,
  onOpenChange,
  onNavigate,
}: CommandPaletteProps) {
  const [query, setQuery] = useState('')
  const [active, setActive] = useState(0)

  const agents = useAgents(api)
  const channels = useChannels(api)
  const runs = useRuns(api, '', '', '')
  const agentNames = useAgentNames(api)

  const wake = useMutation({
    mutationFn: async (agentId: string) => {
      const { error } = await api.POST('/api/v1/agents/{agent_id}/computer/wake', {
        params: { path: { agent_id: agentId } },
      })
      if (error !== undefined) throw new Error('wake failed')
    },
  })

  const destinations = useMemo<Entry[]>(() => {
    const go = (path: string) => () => {
      onOpenChange(false)
      onNavigate(path)
    }
    const items: Entry[] = []
    for (const agent of agents.data ?? []) {
      items.push({
        id: `agent-${agent.id}`,
        label: agent.name,
        hint: 'Sprite',
        run: go(`/sprites/${agent.id}`),
      })
    }
    for (const channel of channels.data ?? []) {
      items.push({
        id: `channel-${channel.id}`,
        label: channel.title ?? 'Conversation',
        hint: 'Conversation',
        run: go(`/c/${channel.id}`),
      })
    }
    for (const run of (runs.data ?? []).slice(0, RUN_LIMIT)) {
      const who = agentNames[run.agent_id] ?? run.agent_id
      items.push({
        id: `run-${run.id}`,
        label: `${who} · ${run.state}`,
        hint: 'Run',
        run: go(`/runs/${run.id}`),
      })
    }
    for (const section of SETTINGS_SECTIONS) {
      items.push({
        id: `settings-${section.value}`,
        label: section.label,
        hint: 'Settings',
        run: go(`/settings/${section.value}`),
      })
    }
    return items
  }, [agents.data, channels.data, runs.data, agentNames, onNavigate, onOpenChange])

  const actions = useMemo<Entry[]>(() => {
    const go = (path: string) => () => {
      onOpenChange(false)
      onNavigate(path)
    }
    const items: Entry[] = [
      {
        id: 'action-new-agent',
        label: 'New sprite',
        hint: 'Action',
        run: go('/sprites?new=1'),
      },
      { id: 'action-open-runs', label: 'Open Runs', hint: 'Action', run: go('/runs') },
      {
        id: 'action-toggle-theme',
        label: 'Toggle theme',
        hint: 'Action',
        run: () => {
          toggleTheme()
          onOpenChange(false)
        },
      },
    ]
    for (const agent of agents.data ?? []) {
      items.push({
        id: `action-wake-${agent.id}`,
        label: `Wake computer for ${agent.name}`,
        hint: 'Action',
        run: () => {
          wake.mutate(agent.id)
          onOpenChange(false)
        },
      })
    }
    return items
  }, [agents.data, wake, onNavigate, onOpenChange])

  // The matches come first and the actions after them.
  const visible = useMemo(
    () => [
      ...destinations.filter((entry) => matches(entry, query)),
      ...actions.filter((entry) => matches(entry, query)),
    ],
    [destinations, actions, query],
  )

  useEffect(() => setActive(0), [query])
  useEffect(() => {
    if (open) {
      setQuery('')
      setActive(0)
    }
  }, [open])

  const move = (delta: number) => {
    if (visible.length === 0) return
    setActive((index) => (index + delta + visible.length) % visible.length)
  }

  const onKeyDown = (event: React.KeyboardEvent) => {
    if (event.key === 'ArrowDown') {
      event.preventDefault()
      move(1)
    } else if (event.key === 'ArrowUp') {
      event.preventDefault()
      move(-1)
    } else if (event.key === 'Enter') {
      event.preventDefault()
      visible[active]?.run()
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title="Command palette">
      <div className="command-palette" onKeyDown={onKeyDown}>
        <Input
          bare
          autoFocus
          value={query}
          aria-label="Search sprites, conversations, runs and commands"
          aria-controls="command-palette-list"
          aria-activedescendant={visible[active]?.id}
          placeholder="Search or run a command…"
          onChange={(event) => setQuery(event.target.value)}
        />
        <ul
          id="command-palette-list"
          className="command-palette-list"
          role="listbox"
          aria-label="Results"
        >
          {visible.map((entry, index) => (
            <li key={entry.id}>
              <Button
                variant="ghost"
                id={entry.id}
                role="option"
                aria-selected={index === active}
                className={
                  index === active
                    ? 'command-palette-item command-palette-item-active'
                    : 'command-palette-item'
                }
                onMouseEnter={() => setActive(index)}
                onClick={() => entry.run()}
              >
                <span className="command-palette-label">{entry.label}</span>
                <span className="command-palette-hint">{entry.hint}</span>
              </Button>
            </li>
          ))}
          {visible.length === 0 && <li className="command-palette-empty">No match</li>}
        </ul>
      </div>
    </Dialog>
  )
}
