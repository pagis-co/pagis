import * as Tabs from '@radix-ui/react-tabs'
import './segmented.css'

export interface SegmentedProps {
  value: string
  onValueChange: (value: string) => void
  items: { value: string; label: string }[]
  label: string
}

export function Segmented({ value, onValueChange, items, label }: SegmentedProps) {
  return (
    <Tabs.Root value={value} onValueChange={onValueChange}>
      <Tabs.List className="ui-segmented" aria-label={label}>
        {items.map((item) => (
          <Tabs.Trigger key={item.value} value={item.value} className="ui-segmented-item">
            {item.label}
          </Tabs.Trigger>
        ))}
      </Tabs.List>
    </Tabs.Root>
  )
}
