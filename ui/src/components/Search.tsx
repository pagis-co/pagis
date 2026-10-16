import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import type { ApiClient } from '../api/client'
import type { components } from '../api/schema'
import { Button, Input } from '../primitives'
import './Search.css'

type SearchItem = components['schemas']['SearchItem']
const kinds: Record<string, string> = { message: 'Message', memory: 'Memory file', calendar: 'Calendar Event', mail: 'Mail' }

export function SearchBox({ onSearch }: { onSearch: (query: string) => void }) {
  const [query, setQuery] = useState('')
  return <form className="global-search" role="search" onSubmit={event => {
    event.preventDefault()
    const text = query.trim()
    if (Array.from(text).length >= 2 && Array.from(text).length <= 200) onSearch(text)
  }}>
    <Input type="search" aria-label="Search everywhere" placeholder="Search everywhere" value={query} maxLength={200} onChange={event => setQuery(event.target.value)} />
    <Button type="submit" aria-label="Search everywhere">Search</Button>
  </form>
}

export function SearchResults({ api, query, onOpen }: { api: ApiClient; query: string; onOpen: (item: SearchItem) => void }) {
  const valid = Array.from(query).length >= 2 && Array.from(query).length <= 200
  const result = useQuery({
    queryKey: ['search', query],
    enabled: valid,
    queryFn: async () => {
      const { data, error } = await api.GET('/api/v1/search', { params: { query: { q: query } } })
      if (data === undefined) throw error ?? new Error('Search failed')
      return data
    },
  })
  return <section className="search-place" aria-label="Search results">
    <h1>Search</h1>
    {!valid ? <p>Enter 2 to 200 characters in Search everywhere.</p> : <>
      <p>Results for “{query}”</p>
      {result.isPending && <p role="status">Searching Workspaces…</p>}
      {result.isError && <p role="alert">Search failed. Try again.</p>}
      {result.data && <>
        {result.data.incomplete.length > 0 && <p role="status">Search is incomplete: {result.data.incomplete.map(workspace => `${workspace.workspace_name} (${workspace.reason})`).join(', ')}. <Button onClick={() => void result.refetch()}>Try again</Button></p>}
        {result.data.items.length === 0 && <p>No results.</p>}
        <ul className="search-results">{result.data.items.map(item => <li key={`${item.workspace.id}:${item.kind}:${item.url}`}>
          <div className="search-labels"><span>{item.workspace.name}</span><span>{item.workspace.kind === 'shared' ? 'Shared Workspace' : 'Private Workspace'}</span><span>{kinds[item.kind] ?? item.kind}</span></div>
          <a href={item.url} onClick={event => { if (event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey) { event.preventDefault(); onOpen(item) } }}>{item.title}</a>
          <p>{item.snippet}</p>
          <time dateTime={new Date(item.at).toISOString()}>{new Date(item.at).toLocaleString()}</time>
        </li>)}</ul>
      </>}
    </>}
  </section>
}
