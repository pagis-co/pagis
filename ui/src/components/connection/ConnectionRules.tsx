// The What reflects section of a connection page. Pagis reads
// the rules in order and the first rule that holds decides. Each rule
// is one sentence here. Edit opens one rule in place, and one
// rule edits at a time.

import { GripVertical } from 'lucide-react'
import { Fragment, useState } from 'react'

import type { ApiClient } from '../../api/client'
import { Badge, Button, Frame, IconButton, Row } from '../../primitives'
import { useFilterPreview } from '../../queries'
import { RuleEditor } from './RuleEditor'
import {
  VERDICT_LABEL,
  startCondition,
  type Catalogue,
  type ReflectionFilter,
  type Rule,
  type Verdict,
} from './rules'
import { RuleSentence } from './RuleSentence'

import './connection.css'

function verdictTone(verdict: Verdict): 'working' | 'neutral' {
  return verdict === 'reflect' ? 'working' : 'neutral'
}

function move(rules: ReflectionFilter['rules'], from: number, to: number) {
  if (to < 0 || to >= rules.length) return rules
  const next = [...rules]
  const [rule] = next.splice(from, 1)
  next.splice(to, 0, rule)
  return next
}

export function ConnectionRules({
  api,
  connectionId,
  catalogue,
  filter,
  backfill,
  onChange,
}: {
  api: ApiClient
  connectionId: string
  catalogue: Catalogue
  filter: ReflectionFilter
  /** How far the backfill has come, for the counts line. */
  backfill?: { reflected: number; pending: number }
  onChange: (filter: ReflectionFilter) => void
}) {
  const preview = useFilterPreview(api, connectionId, filter)
  // The index of the open rule. The index after the last rule is a new
  // rule that is not saved yet.
  const [editing, setEditing] = useState<number>()
  const [dragged, setDragged] = useState<number>()
  const counts = preview.data

  const change = (next: ReflectionFilter) => {
    setEditing(undefined)
    onChange(next)
  }
  const editor = (index: number, rule: Rule) => (
    <RuleEditor
      api={api}
      connectionId={connectionId}
      catalogue={catalogue}
      position={index + 1}
      rule={rule}
      onSave={(saved) => {
        const rules = [...filter.rules]
        rules[index] = saved
        change({ ...filter, rules })
      }}
      onCancel={() => setEditing(undefined)}
      onRemove={() =>
        change({
          ...filter,
          rules: filter.rules.filter((_, at) => at !== index),
        })
      }
    />
  )

  return (
    <Frame>
      {filter.rules.map((rule, index) => (
        <Fragment key={index}>
          {editing === index ? (
            editor(index, rule)
          ) : (
            <Row
              draggable
              onDragStart={() => setDragged(index)}
              onDragOver={(event) => event.preventDefault()}
              onDrop={(event) => {
                event.preventDefault()
                if (dragged !== undefined && dragged !== index) {
                  change({
                    ...filter,
                    rules: move(filter.rules, dragged, index),
                  })
                }
                setDragged(undefined)
              }}
              onDragEnd={() => setDragged(undefined)}
            >
              <IconButton
                icon={GripVertical}
                variant="ghost"
                size="sm"
                className="connection-rule-handle"
                label={`Move rule ${index + 1}`}
                onKeyDown={(event) => {
                  if (event.key !== 'ArrowUp' && event.key !== 'ArrowDown') return
                  event.preventDefault()
                  const to = index + (event.key === 'ArrowUp' ? -1 : 1)
                  change({ ...filter, rules: move(filter.rules, index, to) })
                }}
              />
              <span className="connection-rule-position">{index + 1}.</span>
              <Badge tone={verdictTone(rule.verdict)}>{VERDICT_LABEL[rule.verdict]}</Badge>
              <span className="connection-rule-sentence">
                <RuleSentence catalogue={catalogue} conditions={rule.conditions} />
              </span>
              <span className="connection-rule-actions">
                <Button
                  variant="ghost"
                  aria-label={`Edit rule ${index + 1}`}
                  onClick={() => setEditing(index)}
                >
                  Edit
                </Button>
                <Button
                  variant="ghost"
                  aria-label={`Remove rule ${index + 1}`}
                  onClick={() =>
                    change({
                      ...filter,
                      rules: filter.rules.filter((_, at) => at !== index),
                    })
                  }
                >
                  Remove
                </Button>
              </span>
            </Row>
          )}
        </Fragment>
      ))}
      {editing === filter.rules.length &&
        catalogue.signals.length > 0 &&
        editor(editing, {
          verdict: 'reflect',
          conditions: [startCondition(catalogue.signals[0])],
        })}
      <Row>
        <span className="connection-rule-gap" />
        <span className="connection-rule-position" />
        <Badge tone={verdictTone(filter.default)}>{VERDICT_LABEL[filter.default]}</Badge>
        <span>otherwise</span>
        <span className="connection-row-spacer" />
        <Button
          variant="ghost"
          aria-label="Change the default verdict"
          onClick={() =>
            change({
              ...filter,
              default: filter.default === 'reflect' ? 'skip' : 'reflect',
            })
          }
        >
          Change
        </Button>
      </Row>
      <Row>
        <Button
          disabled={catalogue.signals.length === 0}
          onClick={() => setEditing(filter.rules.length)}
        >
          Add a rule
        </Button>
        <span className="connection-counts">
          {counts === undefined
            ? 'Counting the stored pages…'
            : `Of ${counts.total.toLocaleString()} pages, ${counts.reflect.toLocaleString()} reflect under these rules`}
          {backfill !== undefined &&
            ` · Backfill ${backfill.reflected.toLocaleString()} reflected · ${backfill.pending.toLocaleString()} waiting`}
        </span>
      </Row>
    </Frame>
  )
}
