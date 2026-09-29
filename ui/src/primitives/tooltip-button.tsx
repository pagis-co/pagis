import { forwardRef } from 'react'
import type { ReactNode } from 'react'

import { Button } from './button'
import type { ButtonProps } from './button'
import { Tooltip } from './tooltip'

export interface TooltipButtonProps extends ButtonProps {
  /** The tooltip text. The accessible name comes from the children. */
  tooltip: string
  children: ReactNode
}

/** A button that carries words and a tooltip. `IconButton` covers the
 * controls without words; this covers the ones with them. */
export const TooltipButton = forwardRef<HTMLButtonElement, TooltipButtonProps>(
  function TooltipButton({ tooltip, children, ...rest }, ref) {
    return (
      <Tooltip label={tooltip}>
        <Button ref={ref} {...rest}>
          {children}
        </Button>
      </Tooltip>
    )
  },
)
