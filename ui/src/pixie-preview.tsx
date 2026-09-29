import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'

import Preview from './avatars/preview'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Preview />
  </StrictMode>,
)
