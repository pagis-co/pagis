import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { Button, Frame } from 'pagis-ui/primitives'
import 'pagis-ui/tokens.css'
import 'pagis-ui/fonts'

export const html = renderToStaticMarkup(
  createElement(Frame, null, createElement(Button, { variant: 'primary' }, 'Continue')),
)
