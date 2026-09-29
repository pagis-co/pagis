# Pebble

Pebble follows the supplied Little Sprites illustration. It has a rounded
lavender stone body, mineral flecks, short limbs, and a two-leaf sprout.
The back and sides are interpretations. The model is not an exact copy of
the drawing.

The catalog provides the Classic style and stone and leaf colors. The model
includes the six Pixie clip names and all four facial controls. The stone
moves as one head and body, with separate leaf motion. Celebrate makes one
full turn. No accessory switch is needed.

## Build

Run from the repository root with Blender 5.2:

```sh
blender --background --factory-startup --python assets/avatars/pebble/source/build.py
```

This writes `pebble.blend`, `exports/pebble.glb`, and `portraits/classic.png`.
The source uses `../source/character.py` relative to the avatar family folder.
Run this in a separate Blender process to keep open work intact.

## Inspect

Start the UI development server. Open `/avatar-review.html` to view both
characters with the Pagis renderer. Select each clip and facial expression.
Open an Agent's Appearance tab to check the catalog controls.

The stored Khronos report checks the exported GLB. It is not a visual test.
