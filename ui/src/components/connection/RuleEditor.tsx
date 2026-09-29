// One rule of the What reflects section, opened in place. The
// edits stay a draft until Save rule. The count line previews this one
// rule alone: the pages it decides of all the stored pages.

import { Plus, X } from 'lucide-react'
import { useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Button, IconButton, Row, Select } from '../../primitives'
import { useFilterPreview } from '../../queries'
import {
  OPERATOR_LABELS,
  VERDICT_LABEL,
  operatorsOf,
  startCondition,
  startValue,
  type Catalogue,
  type Condition,
  type Operator,
  type Rule,
  type Verdict,
} from './rules'
import { ValueField } from './ValueField'

import './connection.css'

function ConditionLine({
  catalogue,
  condition,
  position,
  onChange,
  onRemove,
}: {
  catalogue: Catalogue
  condition: Condition
  /** The line number, from 1. */
  position: number
  onChange: (condition: Condition) => void
  onRemove: () => void
}) {
  const signal =
    catalogue.signals.find((item) => item.id === condition.signal) ?? catalogue.signals[0]
  return (
    <div role="group" aria-label={`Condition ${position}`} className="connection-condition">
      <span className="connection-condition-word">{position === 1 ? 'when' : 'and'}</span>
      <Select
        label={`Signal of condition ${position}`}
        value={signal.id}
        onValueChange={(id) => {
          const next = catalogue.signals.find((item) => item.id === id)
          if (next !== undefined) onChange(startCondition(next))
        }}
        items={catalogue.signals.map((item) => ({
          value: item.id,
          label: item.label,
        }))}
      />
      <Select
        label={`Operator of condition ${position}`}
        value={condition.operator}
        onValueChange={(value) => {
          const operator = value as Operator
          onChange({
            ...condition,
            operator,
            value: startValue(signal.kind, operator),
          })
        }}
        items={operatorsOf(signal.kind).map((operator) => ({
          value: operator,
          label: OPERATOR_LABELS[operator],
        }))}
      />
      <ValueField
        position={position}
        signal={signal}
        value={condition.value}
        onChange={(value) => onChange({ ...condition, value })}
      />
      <IconButton
        icon={X}
        variant="ghost"
        size="sm"
        label={`Remove condition ${position}`}
        onClick={onRemove}
      />
    </div>
  )
}

export function RuleEditor({
  api,
  connectionId,
  catalogue,
  position,
  rule,
  onSave,
  onCancel,
  onRemove,
}: {
  api: ApiClient
  connectionId: string
  catalogue: Catalogue
  /** The rule number, from 1. */
  position: number
  rule: Rule
  onSave: (rule: Rule) => void
  onCancel: () => void
  onRemove: () => void
}) {
  const [draft, setDraft] = useState(rule)
  // The rule alone decides the pages it holds for; the default takes
  // the rest. The first per-rule count is the count of this rule.
  const preview = useFilterPreview(api, connectionId, {
    rules: [draft],
    default: draft.verdict === 'skip' ? 'reflect' : 'skip',
  })
  const counts = preview.data
  const setConditions = (conditions: Condition[]) => setDraft({ ...draft, conditions })

  return (
    <Row className="connection-rule-editor">
      <div className="connection-rule-editor-line">
        <span className="connection-rule-gap" />
        <span className="connection-rule-position">{position}.</span>
        <Select
          label={`Verdict of rule ${position}`}
          value={draft.verdict}
          onValueChange={(verdict) => setDraft({ ...draft, verdict: verdict as Verdict })}
          items={(['reflect', 'skip'] as const).map((verdict) => ({
            value: verdict,
            label: VERDICT_LABEL[verdict],
          }))}
        />
        <span className="connection-hint">this page when every condition holds</span>
      </div>
      <div className="connection-rule-editor-body">
        {draft.conditions.map((condition, index) => (
          <ConditionLine
            key={index}
            catalogue={catalogue}
            condition={condition}
            position={index + 1}
            onChange={(next) =>
              setConditions(draft.conditions.map((held, at) => (at === index ? next : held)))
            }
            onRemove={() => setConditions(draft.conditions.filter((_, at) => at !== index))}
          />
        ))}
        <div className="connection-condition">
          <span className="connection-condition-word" />
          <Button
            variant="ghost"
            disabled={catalogue.signals.length === 0}
            onClick={() =>
              setConditions([...draft.conditions, startCondition(catalogue.signals[0])])
            }
          >
            <Plus size={14} aria-hidden focusable="false" />
            Add a condition
          </Button>
          <span className="connection-counts">
            {counts === undefined ? (
              'Counting the stored pages…'
            ) : (
              <>
                would {draft.verdict} <strong>{(counts.rules[0] ?? 0).toLocaleString()}</strong> of{' '}
                {counts.total.toLocaleString()} pages
              </>
            )}
          </span>
        </div>
      </div>
      <div className="connection-rule-editor-body connection-rule-editor-actions">
        <Button variant="primary" onClick={() => onSave(draft)}>
          Save rule
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
        <Button variant="ghost" className="connection-row-spacer" onClick={onRemove}>
          Remove rule
        </Button>
      </div>
    </Row>
  )
}
