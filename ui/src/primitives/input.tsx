import { forwardRef } from 'react'
import type { InputHTMLAttributes, TextareaHTMLAttributes } from 'react'

import { cx } from './cx'
import './input.css'

/** A field that sits in a card which already draws a frame shows no
 * frame of its own: no border, no background and no radius. */
interface BareProp {
  bare?: boolean
}

export type InputProps = InputHTMLAttributes<HTMLInputElement> & BareProp

/** A single-line field. */
export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { className, type, bare, ...rest },
  ref,
) {
  return (
    <input
      ref={ref}
      type={type ?? 'text'}
      className={cx('ui-input', bare && 'ui-input-bare', className)}
      {...rest}
    />
  )
})

export type TextareaProps = TextareaHTMLAttributes<HTMLTextAreaElement> & BareProp

/** A multi-line field. It shares the field styling of `Input`. */
export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(
  function Textarea({ className, bare, ...rest }, ref) {
    return (
      <textarea
        ref={ref}
        className={cx('ui-input', 'ui-textarea', bare && 'ui-input-bare', className)}
        {...rest}
      />
    )
  },
)
