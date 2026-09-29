// The value part of one condition line. The field follows the
// value kind: chips for a list, a number field, a date, a checkbox.

import { X } from 'lucide-react'

import { Badge, IconButton, Input, Select } from '../../primitives'
import { acceptsEntry, optionsOf, type ConditionValue, type Signal } from './rules'

function pad(part: number) {
  return String(part).padStart(2, '0')
}

/** The local calendar day of a moment, as a date field holds it. */
function dayOf(at: number) {
  const date = new Date(at)
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}`
}

/** The local midnight that starts a calendar day. */
function startOf(day: string) {
  const [year, month, date] = day.split('-').map(Number)
  return new Date(year, month - 1, date).getTime()
}

function ChipEntry({
  label,
  signal,
  values,
  onChange,
}: {
  label: string
  signal: Signal
  values: string[]
  onChange: (values: string[]) => void
}) {
  const kind = signal.kind
  const hint =
    kind.kind === 'list' && kind.format === 'email'
      ? 'name@example.com'
      : kind.kind === 'list' && kind.format === 'domain'
        ? 'example.com'
        : 'value'
  return (
    <span className="connection-chips">
      {values.map((value, index) => (
        <Badge key={`${value}-${index}`} className="connection-chip">
          {value}
          <IconButton
            icon={X}
            variant="ghost"
            size="sm"
            label={`Remove ${value}`}
            onClick={() => onChange(values.filter((_, at) => at !== index))}
          />
        </Badge>
      ))}
      <Input
        bare
        aria-label={label}
        placeholder={hint}
        onKeyDown={(event) => {
          if (event.key !== 'Enter') return
          event.preventDefault()
          const entry = event.currentTarget.value.trim()
          if (!acceptsEntry(kind, entry) || values.includes(entry)) return
          onChange([...values, entry])
          event.currentTarget.value = ''
        }}
      />
    </span>
  )
}

export function ValueField({
  position,
  signal,
  value,
  onChange,
}: {
  /** The line number, from 1, that names the fields. */
  position: number
  signal: Signal
  value: ConditionValue
  onChange: (value: ConditionValue) => void
}) {
  const label = `Value of condition ${position}`
  switch (value.kind) {
    case 'list':
      return (
        <ChipEntry
          label={`Values of condition ${position}`}
          signal={signal}
          values={value.values}
          onChange={(values) => onChange({ kind: 'list', values })}
        />
      )
    case 'choices':
      return (
        <span className="connection-chips">
          {optionsOf(signal.kind).map((option) => (
            <label key={option} className="connection-check">
              <input
                type="checkbox"
                checked={value.values.includes(option)}
                onChange={(event) =>
                  onChange({
                    kind: 'choices',
                    values: event.target.checked
                      ? [...value.values, option]
                      : value.values.filter((held) => held !== option),
                  })
                }
              />
              {option}
            </label>
          ))}
        </span>
      )
    case 'choice':
      return (
        <Select
          label={label}
          value={value.value}
          onValueChange={(option) => onChange({ kind: 'choice', value: option })}
          items={optionsOf(signal.kind).map((option) => ({
            value: option,
            label: option,
          }))}
        />
      )
    case 'duration':
      return (
        <>
          <Input
            type="number"
            min={1}
            aria-label={label}
            className="connection-number"
            value={value.amount}
            onChange={(event) =>
              onChange({
                ...value,
                amount: Math.max(1, Number(event.target.value) || 1),
              })
            }
          />
          <Select
            label={`Unit of condition ${position}`}
            value={value.unit}
            onValueChange={(unit) => onChange({ ...value, unit: unit as 'days' | 'hours' })}
            items={[
              { value: 'days', label: 'days' },
              { value: 'hours', label: 'hours' },
            ]}
          />
        </>
      )
    case 'date_time':
      return (
        <Input
          type="date"
          aria-label={label}
          value={dayOf(value.at)}
          onChange={(event) => {
            if (event.target.value) onChange({ kind: 'date_time', at: startOf(event.target.value) })
          }}
        />
      )
    case 'number':
      return (
        <Input
          type="number"
          aria-label={label}
          className="connection-number"
          value={value.value}
          onChange={(event) => onChange({ kind: 'number', value: Number(event.target.value) || 0 })}
        />
      )
    case 'boolean':
      return (
        <label className="connection-check">
          <input
            type="checkbox"
            checked={value.value}
            onChange={(event) => onChange({ kind: 'boolean', value: event.target.checked })}
          />
          {signal.label}
        </label>
      )
    case 'text':
      return (
        <Input
          aria-label={label}
          value={value.value}
          onChange={(event) => onChange({ kind: 'text', value: event.target.value })}
        />
      )
  }
}
