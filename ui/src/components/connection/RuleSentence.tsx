// The read state of one rule's conditions: "when Labels has
// IMPORTANT and Messages is at most 1". The values are strong. A rule
// with no condition holds for every page, so it reads "always".

import { Fragment } from 'react'

import type { components } from '../../api/schema'
import { OPERATOR_LABELS, type Catalogue } from './rules'

type Condition = components['schemas']['Condition']
type ConditionValue = components['schemas']['ConditionValue']

function Value({ value }: { value: ConditionValue }) {
  switch (value.kind) {
    case 'list':
    case 'choices':
      return value.values.map((entry, index) => (
        <Fragment key={`${entry}-${index}`}>
          {index > 0 && ', '}
          <strong>{entry}</strong>
        </Fragment>
      ))
    case 'duration':
      return (
        <strong>
          {value.amount} {value.unit}
        </strong>
      )
    case 'date_time':
      return <strong>{new Date(value.at).toLocaleDateString()}</strong>
    case 'boolean':
      // The signal is the sentence: "The owner replied", "not The owner replied".
      return null
    default:
      return <strong>{String(value.value)}</strong>
  }
}

export function RuleSentence({
  catalogue,
  conditions,
}: {
  catalogue: Catalogue
  conditions: Condition[]
}) {
  if (conditions.length === 0) return <>always</>
  return (
    <>
      when{' '}
      {conditions.map((condition, index) => {
        const label =
          catalogue.signals.find((signal) => signal.id === condition.signal)?.label ??
          condition.signal
        const boolean = condition.value.kind === 'boolean' ? condition.value : undefined
        return (
          <Fragment key={index}>
            {index > 0 && ' and '}
            {boolean !== undefined && !boolean.value && 'not '}
            {label}
            {boolean === undefined && ` ${OPERATOR_LABELS[condition.operator]} `}
            <Value value={condition.value} />
          </Fragment>
        )
      })}
    </>
  )
}
