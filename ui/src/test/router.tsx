// A router around one component under test. A row that links to
// a run needs the router that owns `/runs`, and the Thread header links
// the Agent at `/agents`; the test router holds the component and a
// stand-in view for each, so a click on the link is answered in the
// same document and nothing reloads.

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
  const router = createRouter({
    routeTree: rootRoute.addChildren([indexRoute, runRoute, agentRoute]),
    history,
  })
  render(<RouterProvider router={router as never} />)
  return history
}
