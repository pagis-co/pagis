// A router around one component under test. A row that links to
// a run needs the router that owns `/runs`, the Thread header links
// the Agent at `/sprites`, the session page links its Thread, and the
// session block links the session page; the test router holds the
// component and a stand-in view for each, so a click on the link is
// answered in the same document and nothing reloads.

import {
  Outlet,
  RouterProvider,
  createMemoryHistory,
  createRootRoute,
  createRoute,
  createRouter,
  type RouterHistory,
} from '@tanstack/react-router'
import { render } from '@testing-library/react'
import type { ReactNode } from 'react'

export function renderInRouter(ui: ReactNode): RouterHistory {
  const history = createMemoryHistory({ initialEntries: ['/'] })
  const rootRoute = createRootRoute({ component: () => <Outlet /> })
  const indexRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/',
    component: () => <>{ui}</>,
  })
  const runRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/runs/$runId',
    component: () => <div data-testid="run-view">the run</div>,
  })
  const agentRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/sprites/$agentId',
    component: () => <div data-testid="agent-view">the agent</div>,
  })
  const threadRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/c/$channelId/t/$messageId',
    component: () => <div data-testid="thread-view">the thread</div>,
  })
  const codingRoute = createRoute({
    getParentRoute: () => rootRoute,
    path: '/coding/$sessionId',
    component: () => <div data-testid="coding-view">the coding session</div>,
  })
  const router = createRouter({
    routeTree: rootRoute.addChildren([
      indexRoute,
      runRoute,
      agentRoute,
      threadRoute,
      codingRoute,
    ]),
    history,
  })
  render(<RouterProvider router={router as never} />)
  return history
}
