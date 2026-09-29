import { useId } from 'react'
import {
  spriteCatalog,
  spriteAsset,
  defaultAppearance,
  type SpriteAppearance,
} from './catalog'
import { Input, Select } from '../primitives'

export function AppearanceControls({
  value,
  onChange,
  compact = false,
}: {
  value: SpriteAppearance
  onChange: (value: SpriteAppearance) => void
  compact?: boolean
}) {
  const id = useId()
  const definition = spriteCatalog[value.sprite]
  const preset = definition.presets[value.preset]
  return (
    <div className="appearance-controls">
      <Select
        label="Sprite"
        value={value.sprite}
        onValueChange={(sprite) => onChange(defaultAppearance(sprite))}
        items={Object.entries(spriteCatalog).map(([value, sprite]) => ({
          value,
          label: sprite.label,
        }))}
      />
      <fieldset className="sprite-presets">
        <legend>Style</legend>
        {Object.entries(definition.presets).map(([key, preset]) => (
          <label key={key}>
            <input
              type="radio"
              name={id}
              checked={value.preset === key}
              onChange={() =>
                onChange({ ...defaultAppearance(value.sprite), preset: key })
              }
            />
            <img src={spriteAsset(preset.portrait)} alt="" />
            <span>{preset.label}</span>
          </label>
        ))}
      </fieldset>
      {!compact && (
        <>
          <fieldset className="sprite-colors">
            <legend>Colors</legend>
            {Object.entries(definition.colors).map(([key, slot]) => (
              <label key={key}>
                {slot.label}
                <Input
                  type="color"
                  aria-label={`${slot.label} color`}
                  value={
                    value.colors[key] ?? preset.materials[slot.materials[0]]
                  }
                  onInput={(event) =>
                    onChange({
                      ...value,
                      colors: {
                        ...value.colors,
                        [key]: event.currentTarget.value,
                      },
                    })
                  }
                />
              </label>
            ))}
          </fieldset>
          <fieldset className="sprite-accessories">
            <legend>Accessories</legend>
            {Object.entries(definition.accessoryLabels).map(([key, label]) => (
              <label key={key}>
                <input
                  type="checkbox"
                  checked={value.accessories[key] ?? preset.accessories[key]}
                  onChange={(event) =>
                    onChange({
                      ...value,
                      accessories: {
                        ...value.accessories,
                        [key]: event.target.checked,
                      },
                    })
                  }
                />
                {label}
              </label>
            ))}
          </fieldset>
        </>
      )}
    </div>
  )
}
