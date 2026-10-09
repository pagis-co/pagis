// The choice card block (ADR-0004): one tap, like an
// approval card. It is its own type rather than a form with one enum
// field because the two differ in interaction and not in data. State
// and the chosen value come from the Request row. A settled choice
// collapses to one line that names the chosen option and the state.

import { useState } from 'react'
import { useIsMobile } from '../state/useIsMobile'
import { ListChecks } from 'lucide-react'

import type { ApiClient } from '../api/client'
import { Button, Frame, Row } from '../primitives'
import { errorMessage, useDecideRequest } from '../queries'
import { Card, CardBody, CardFooter, CardHeader, SettledLine } from './Card'
import type { SettledTone } from './Card'
import { STATE_LABEL, optionsOf, useQuestion, type ChoiceOption } from './question'

import './request.css'

const STATE_TONE: Record<string, SettledTone> = {
  approved: 'working',
  denied: 'failed',
}

export function ChoiceCard({
  api,
  requestId,
  title,
  body,
  options: denormalized,
}: {
  api: ApiClient
  requestId: string
  title: string
  body: string | null | undefined
  options: ChoiceOption[]
}) {
  const phone = useIsMobile()
  const [selected, setSelected] = useState<string | null>(null)
  const question = useQuestion(api, requestId)
  const decide = useDecideRequest(api, requestId)
  const options = optionsOf(question.payload, denormalized)
  const chosen = question.values?.value

  if (question.settled) {
    const state = question.state!
    const label = options.find((option) => option.value === chosen)?.label
    return (
      <Card className={`choice-${state}`} data-testid="choice-settled">
        <SettledLine
          tone={STATE_TONE[state] ?? 'neutral'}
          aside={STATE_LABEL[state] ?? state}
        >
          {title}
          {label !== undefined && (
            <span className="choice-settled-chosen"> · {label}</span>
          )}
        </SettledLine>
      </Card>
    )
  }

  if (phone)
    return (
      <Frame data-testid="choice-card">
        <Row>
          <strong>{title}</strong>
        </Row>
        {body && <Row>{body}</Row>}
        {options.map((option) => (
          <Row key={option.value}>
            <label className="phone-choice-row">
              <input
                type="radio"
                name={`choice-${requestId}`}
                checked={selected === option.value}
                disabled={!question.pending || decide.isPending}
                onChange={() => setSelected(option.value)}
              />
              <span className="phone-row-copy">
                <strong>{option.label}</strong>
                {option.description && <span className="phone-hint">{option.description}</span>}
              </span>
            </label>
          </Row>
        ))}
        <Row>
          <div className="phone-actions phone-choice-submit">
            <Button
              disabled={!question.pending || decide.isPending}
              onClick={() => decide.mutate({ decision: 'denied' })}
            >
              Dismiss
            </Button>
            <Button
              variant="primary"
              disabled={!selected || !question.pending || decide.isPending}
              onClick={() => decide.mutate({ decision: 'approved', values: { value: selected } })}
            >
              Submit
            </Button>
          </div>
        </Row>
        {decide.isError && (
          <Row role="alert">{errorMessage(decide.error, 'The daemon refused this answer.')}</Row>
        )}
      </Frame>
    )

  return (
    <Card className="choice-card" data-testid="choice-card">
      <CardHeader
        icon={ListChecks}
        act={title}
        place={body !== null && body !== undefined && body !== '' ? body : undefined}
      />
      <CardBody className="choice-card-options">
        {options.map((option) => (
          <Button
            key={option.value}
            variant="ghost"
            size="lg"
            className="choice-option"
            disabled={!question.pending || decide.isPending}
            onClick={() =>
              decide.mutate({
                decision: 'approved',
                values: { value: option.value },
              })
            }
          >
            <span className="choice-option-label">{option.label}</span>
            {option.description != null && option.description !== '' && (
              <span className="choice-option-description">{option.description}</span>
            )}
          </Button>
        ))}
      </CardBody>
      {(question.pending || decide.isError) && (
        <CardFooter>
          {decide.isError && (
            <span className="request-form-error" role="alert">
              {errorMessage(decide.error, 'The daemon refused this answer.')}
            </span>
          )}
          {question.pending && (
            <Button
              variant="ghost"
              className="request-dismiss"
              disabled={decide.isPending}
              onClick={() => decide.mutate({ decision: 'denied' })}
            >
              Dismiss
            </Button>
          )}
        </CardFooter>
      )}
    </Card>
  )
}
