// What a `form` and a `choice_card` share (ADR-0004): both view a
// Request row, and the rule of `approval_card` holds without
// exception. The block carries `request_id` plus denormalized display
// fields so the message renders standalone; the live state, the
// schema the daemon validates against, and the submitted answer all
// come from the row.

import type { ApiClient } from '../api/client'
import type { components } from '../api/schema'
import { useRequest } from '../queries'

export type FormField = components['schemas']['FormField']
export type ChoiceOption = components['schemas']['ChoiceOption']

/** What a settled Request says about itself, in the user's words. */
export const STATE_LABEL: Record<string, string> = {
  approved: 'Answered',
  denied: 'Dismissed',
  // The waiting run died before anyone answered (ADR-0004).
  expired: 'Expired — the run ended before you answered',
  // The user sent a message in place of an answer.
  superseded: 'You replied instead',
}

export type Question = {
  /** The row's own payload, or `undefined` until it loads. */
  payload: Record<string, unknown> | undefined
  state: string | undefined
  values: Record<string, unknown> | undefined
  /** Interactive only while the row says pending. A row that has not
   *  loaded is never interactive: the block alone is not evidence. */
  pending: boolean
  settled: boolean
}

export function useQuestion(api: ApiClient, requestId: string): Question {
  const request = useRequest(api, requestId)
  const state = request.data?.state
  return {
    payload: request.data?.payload as Record<string, unknown> | undefined,
    state,
    values: (request.data?.values ?? undefined) as
      | Record<string, unknown>
      | undefined,
    pending: state === 'pending',
    settled: state !== undefined && state !== 'pending',
  }
}

/** The row's field list, falling back to the block's copy while the
 *  row loads. The fallback is display only: a submission is always
 *  validated against the row, here and again on the daemon. */
export function fieldsOf(
  payload: Record<string, unknown> | undefined,
  fallback: FormField[],
): FormField[] {
  const fields = payload?.fields
  return Array.isArray(fields) ? (fields as FormField[]) : fallback
}

export function optionsOf(
  payload: Record<string, unknown> | undefined,
  fallback: ChoiceOption[],
): ChoiceOption[] {
  const options = payload?.options
  return Array.isArray(options) ? (options as ChoiceOption[]) : fallback
}

/** Check one submission against the same schema the daemon uses, so a
 *  refusal names its field before a round trip. The daemon checks it
 *  again against the row; this never replaces that. */
export function validate(
  fields: FormField[],
  values: Record<string, unknown>,
): Record<string, string> {
  const errors: Record<string, string> = {}
  for (const field of fields) {
    const value = values[field.key]
    const empty = value === undefined || value === '' || value === null
    if (empty) {
      if (field.required === true) errors[field.key] = 'This is required.'
      continue
    }
    if (field.kind === 'number' && Number.isNaN(Number(value))) {
      errors[field.key] = 'Enter a number.'
    }
    if (
      field.kind === 'select' &&
      !(field.options ?? []).some((option) => option.value === value)
    ) {
      errors[field.key] = 'Choose one of the options.'
    }
  }
  return errors
}

/** The typed value one field submits: the daemon's field kinds are
 *  primitives, and the row's schema refuses anything else. */
export function submitted(
  fields: FormField[],
  values: Record<string, unknown>,
): Record<string, unknown> {
  const body: Record<string, unknown> = {}
  for (const field of fields) {
    const value = values[field.key]
    if (value === undefined || value === '' || value === null) continue
    body[field.key] = field.kind === 'number' ? Number(value) : value
  }
  return body
}
