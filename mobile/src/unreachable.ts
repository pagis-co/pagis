/** The bundled Unreachable screen of the Mobile App. */

import '@fontsource-variable/inter'

import { PagisShell } from './shell'
import { mountUnreachableScreen } from './unreachableScreen'
import './connect.css'

mountUnreachableScreen(document, location.search, {
  open: (address) => PagisShell.open(address),
  changeServer: () => PagisShell.changeServer(),
})
