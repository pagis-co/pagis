// The words and the start values of the Reflection Filter (ADR-0011).
// The rules are ordered; the first rule whose conditions all hold decides.

import type { components } from '../../api/schema'

export type ReflectionFilter = components['schemas']['ReflectionFilter']
export type Catalogue = components['schemas']['Catalogue']
export type Signal = components['schemas']['Signal']
export type ValueKind = components['schemas']['ValueKind']
export type Rule = components['schemas']['Rule']
export type Condition = components['schemas']['Condition']
export type ConditionValue = components['schemas']['ConditionValue']
export type Operator = components['schemas']['Operator']
export type Verdict = components['schemas']['Verdict']

export const VERDICT_LABEL: Record<Verdict, string> = {
  reflect: 'Reflect',
  skip: 'Skip',
}

export const OPERATOR_LABELS: Record<Operator, string> = {
  contains: 'contains',
  not_contains: 'does not contain',
  is: 'is',
  is_not: 'is not',
  starts_with: 'starts with',
  after: 'is after',
  before: 'is before',
  within_last: 'is within the last',
  older_than: 'is older than',
  at_least: 'is at least',
  at_most: 'is at most',
  in: 'is in',
  not_in: 'is not in',
  has: 'has',
  lacks: 'lacks',
}

/** The operators one kind allows. It mirrors `ValueKind::operators`. */
export function operatorsOf(kind: ValueKind): Operator[] {
  switch (kind.kind) {
    case 'text':
    case 'list':
      return ['contains', 'not_contains', 'is', 'starts_with', 'in', 'not_in']
    case 'date_time':
      return ['after', 'before', 'within_last', 'older_than']
    case 'duration':
      return ['within_last', 'older_than']
    case 'number':
      return ['at_least', 'at_most', 'is']
    case 'boolean':
      return ['is']
    case 'choice':
      return ['is', 'is_not', 'in']
    case 'tag_set':
      return ['has', 'lacks']
  }
}

/** The fixed options of a choice or a tag set. Other kinds have none. */
export function optionsOf(kind: ValueKind): string[] {
  return kind.kind === 'choice' || kind.kind === 'tag_set' ? kind.options : []
}

/** Whether one list entry has the form the signal names. */
export function acceptsEntry(kind: ValueKind, entry: string): boolean {
  const value = entry.trim()
  if (!value) return false
  if (kind.kind !== 'list') return true
  if (kind.format === 'email') return /^[^\s@]+@[^\s@.]+\.[^\s@]+$/.test(value)
  if (kind.format === 'domain') return /^[^\s@.]+(\.[^\s@.]+)+$/.test(value)
  return true
}

/** The value one signal and operator start from. */
export function startValue(kind: ValueKind, operator: Operator): ConditionValue {
  if (operator === 'within_last' || operator === 'older_than') {
    return { kind: 'duration', amount: 30, unit: 'days' }
  }
  if (operator === 'in' || operator === 'not_in') {
    const options = optionsOf(kind)
    return options.length ? { kind: 'choices', values: [options[0]] } : { kind: 'list', values: [] }
  }
  switch (kind.kind) {
    case 'date_time':
      return { kind: 'date_time', at: Date.now() }
    case 'number':
      return { kind: 'number', value: 1 }
    case 'boolean':
      return { kind: 'boolean', value: true }
    case 'choice':
    case 'tag_set':
      return { kind: 'choice', value: optionsOf(kind)[0] ?? '' }
    default:
      return { kind: 'text', value: '' }
  }
}

/** A condition on one signal, with the first operator of its kind. */
export function startCondition(signal: Signal): Condition {
  const operator = operatorsOf(signal.kind)[0]
  return {
    signal: signal.id,
    operator,
    value: startValue(signal.kind, operator),
  }
}
