// The entry point of the Administration Interface. The daemon
// serves this page on the administration port, and the product page on
// the product port; both are entries of this one package, so every
// component they share is one component.

import { QueryClientProvider } from '@tanstack/react-query'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import { createQueryClient } from '../queries'
import { Administration } from './Administration'
import '@fontsource-variable/inter'
import '../tokens.css'
import '../styles.css'
import './administration.css'

const queryClient = createQueryClient()

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <Administration />
    </QueryClientProvider>
  </StrictMode>,
)
