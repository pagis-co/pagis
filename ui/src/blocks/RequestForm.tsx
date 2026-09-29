// The form block (ADR-0004): fill then submit, the answer
// going to the run that asked through the same decision endpoint an
// approval uses. A submission is not a message — the timeline never
// needs a second row to explain what happened, which is why a settled
// form re-renders here read-only with its values.

import { useState } from 'react'
import { FileText } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Button, Input, Select } from '../primitives'
import { errorMessage, useDecideRequest } from '../queries'
import { Card, CardBody, CardFooter, CardHeader } from './Card'
import {
  STATE_LABEL,
  fieldsOf,
  submitted,
  useQuestion,
  validate,
  type FormField,
} from './question'

import './request.css'

function fieldInput({
  field,
  value,
  disabled,
  onChange,
}: {
  field: FormField
  value: unknown
  disabled: boolean
  onChange: (value: unknown) => void
}) {
  const id = `field-${field.key}`
  switch (field.kind) {
    case 'select':
      // A Radix trigger is a button, which the wrapping label does not
      // name, so the field label names it here as well.
      return (
        <Select
          id={id}
          label={field.label}
          placeholder="Choose…"
          disabled={disabled}
          value={(value as string) ?? ''}
          onValueChange={onChange}
          items={(field.options ?? []).map((option) => ({
            value: option.value,
            label: option.label,
          }))}
        />
      )
    case 'checkbox':
      return (
        <input
          id={id}
          type="checkbox"
          disabled={disabled}
          checked={value === true}
          onChange={(event) => onChange(event.target.checked)}
        />
      )
    default:
      return (
        <Input
          id={id}
          type={
            field.kind === 'number'
              ? 'number'
              : field.kind === 'date'
                ? 'date'
                : 'text'
          }
          disabled={disabled}
          placeholder={field.placeholder ?? undefined}
          value={(value as string | number) ?? ''}
          onChange={(event) => onChange(event.target.value)}
        />
      )
  }
}

export function RequestForm({
  api,
  requestId,
  title,
  fields: denormalized,
  submitLabel,
}: {
  api: ApiClient
  requestId: string
  title: string
  fields: FormField[]
  submitLabel: string | null | undefined
}) {
  const question = useQuestion(api, requestId)
  const decide = useDecideRequest(api, requestId)
  const [draft, setDraft] = useState<Record<string, unknown>>({})
  const [errors, setErrors] = useState<Record<string, string>>({})

  const fields = fieldsOf(question.payload, denormalized)
  const disabled = !question.pending || decide.isPending
  // A settled form shows what was submitted, read from the row.
  const shown = question.settled ? (question.values ?? {}) : draft

  const submit = () => {
    const found = validate(fields, draft)
    setErrors(found)
    if (Object.keys(found).length > 0) return
    decide.mutate({ decision: 'approved', values: submitted(fields, draft) })
  }

  return (
    <Card className="request-form-card" data-testid="form-block">
      <form
        className="request-form"
        onSubmit={(event) => {
          event.preventDefault()
          submit()
        }}
      >
        <CardHeader icon={FileText} act={title} />
        <CardBody className="request-form-fields">
          {fields.map((field) => (
            <div className="request-form-field" key={field.key}>
              <label htmlFor={`field-${field.key}`}>
                {field.label}
                {field.required === true && (
                  <span className="request-form-required" aria-hidden="true">
                    {' '}
                    *
                  </span>
                )}
              </label>
              {fieldInput({
                field,
                value: shown[field.key],
                disabled,
                onChange: (value) => {
                  setDraft((current) => ({ ...current, [field.key]: value }))
                  setErrors(({ [field.key]: _cleared, ...rest }) => rest)
                },
              })}
              {errors[field.key] !== undefined && (
                <span className="request-form-error" role="alert">
                  {errors[field.key]}
                </span>
              )}
            </div>
          ))}
        </CardBody>
        <CardFooter>
          {question.pending && (
            <>
              <Button variant="primary" type="submit" disabled={decide.isPending}>
                {submitLabel ?? 'Submit'}
              </Button>
              <Button
                variant="ghost"
                className="request-dismiss"
                disabled={decide.isPending}
                onClick={() => decide.mutate({ decision: 'denied' })}
              >
                Dismiss
              </Button>
            </>
          )}
          {/* The daemon validates against the row, never against this
              block, so its refusal is the last word. The run stays parked. */}
          {decide.isError && (
            <span className="request-form-error" role="alert">
              {errorMessage(decide.error, 'The daemon refused this answer.')}
            </span>
          )}
          {question.settled && (
            <span className={`request-state request-${question.state}`}>
              {STATE_LABEL[question.state!] ?? question.state}
            </span>
          )}
        </CardFooter>
      </form>
    </Card>
  )
}
