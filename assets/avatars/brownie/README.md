# Brownie

Brownie follows the supplied Little Sprites illustration. It has a rust cap,
cream face, tan tunic, resting hands, and an optional acorn satchel. The back
and sides are interpretations. The model is not an exact copy of the drawing.

The catalog provides the Classic style, hat and tunic colors, and a satchel
switch. The model includes the six Pixie clip names and all four facial
controls. Hands stay still. Celebrate makes one full turn.

## Build

Run from the repository root with Blender 5.2:

```sh
blender --background --factory-startup --python assets/avatars/brownie/source/build.py
```

This writes `brownie.blend`, `exports/brownie.glb`, and `portraits/classic.png`.
The source uses `../source/character.py` relative to the avatar family folder.
Run this in a separate Blender process to keep open work intact.

## Inspect

Start the UI development server. Open `/avatar-review.html` to view Brownie
and Pebble with the Pagis renderer. Select each clip and facial expression.
Open an Agent's Appearance tab to check the catalog controls.

The stored Khronos report checks the exported GLB. It is not a visual test.
