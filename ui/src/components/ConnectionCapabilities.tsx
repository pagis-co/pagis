// Named capabilities shared by Connection authorization and Agent access.

import './ConnectionCapabilities.css'

export const CONNECTION_CAPABILITIES: { name: string; label: string }[] = [
  { name: 'gmail_read', label: 'Read Gmail' },
  { name: 'calendar_read', label: 'Read Calendar' },
  { name: 'gmail_send', label: 'Send email' },
  { name: 'gmail_modify', label: 'Change mail state' },
  { name: 'calendar_write', label: 'Change Calendar' },
]

export const READ_ONLY_CONNECTION_CAPABILITIES = [
  'gmail_read',
  'calendar_read',
]

export function CapabilityPicker({
  selected,
  onChange,
}: {
  selected: string[]
  onChange: (capabilities: string[]) => void
}) {
  return (
    <fieldset className="connection-capabilities">
      <legend>What may sprites do with this account?</legend>
      {CONNECTION_CAPABILITIES.map((capability) => (
        <label key={capability.name}>
          <input
            type="checkbox"
            checked={selected.includes(capability.name)}
            onChange={(event) =>
              onChange(
                event.target.checked
                  ? [...selected, capability.name]
                  : selected.filter((name) => name !== capability.name),
              )
            }
          />
          <span>{capability.label}</span>
        </label>
      ))}
    </fieldset>
  )
}
