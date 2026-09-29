# Pixie

This asset follows the Pixie in `source/reference.png`, an illustration
generated for this project. The back and side views are an interpretation of
that front view. The water, brownie, wisp, pebble, and star characters are not
part of this asset.

## Shape

The small open palms sit beside the cheeks. Each hand has four short rounded
fingers and an inward-facing thumb. Soft wrist joins and short tapered sleeves
connect them to the body. The hands have fixed Body weights and no separate
controls. The wide upper body supports the head without a visible neck.
Long pointed ears, head tilts, blinks, and a full spin carry its expressions.
The flat satchel strap follows the body. The hair has curved leaves
with fine veins. The three colorways share the shape and the controls.
The browser uses local studio lights and reflections; it needs no remote
lighting file. The render remains a stylized interpretation of the illustration.

## Files

| File | Use |
| --- | --- |
| `pixie.blend` | Edit the model, skeleton, materials, expressions, lights, and clips. |
| `exports/pixie.glb` | Use all parts in an application with appearance controls. |
| `exports/pixie-mint.glb` | Open the mint character with its satchel in a normal viewer. |
| `exports/pixie-lavender.glb` | Open lavender with glasses and a satchel. |
| `exports/pixie-peach.glb` | Open peach with a scarf and a satchel. |
| `portraits/mint.png`, `lavender.png`, `peach.png` | Use transparent 900 by 1000 portraits. |
| `portraits/mint-studio.png` | View the model with a studio ground and contact shadow. |
| `../catalog.json` | Read the color presets, accessory defaults, attachment names, and clip durations. |
| `source/*.py` | Build, animate, render, and export through Blender. |
| `tests/test_export.py` | Check the exported GLB contract. |
| `tests/validator-report.json` | Read the Khronos validator results for all four exports. |

The full GLB is about 7.7 MB. It has 8 bones, 133,964 triangles, 19 materials,
and 20 draw calls with all accessories present. One 512 by 512 normal texture
is embedded in the file. It has no external texture dependency.
Pagis runs one visible animated avatar and uses generated still portraits in lists.

The full GLB includes every accessory. glTF does not have a standard visibility
flag that all viewers support. The Pagis loader applies the selected preset.
Use a named colorway export when the target viewer has no appearance controls.

## Preview

From `ui`, run `npm run dev -- --host 127.0.0.1 --port 5187` and open
`http://127.0.0.1:5187/pixie.html`. Drag the character to turn it. The controls
select a preset, change colors, show accessories, play clips, and set expressions.
The studio includes a side-by-side view of the illustration and the current
Blender render. It is a development preview, not a new production route.

The studio download link returns the full base GLB. It does not bake the current
preview settings. Use the named exports, or edit the Blender file and export it,
to make a fixed appearance for another application.

## Animation and expression controls

| Clip | Duration | Playback |
| --- | --- | --- |
| `Idle` | 4 seconds | Repeat |
| `Working` | 3 seconds | Repeat |
| `Waiting` | 5 seconds | Repeat |
| `NeedsInput` | 3.2 seconds | Repeat |
| `Celebrate` | 2.4 seconds | Once, then return to Idle |
| `Error` | 3 seconds | Once, then return to Idle |

The clips animate only the head, ears, and whole-avatar rotation. Hands, body,
feet, and sprout have no independent motion. Celebrate makes one full turn.
These states do not depend on limbs and can guide the other sprite families;
those families still need their own models and controls.

`Blink`, `Smile`, `Surprise`, and `Concern` are
separate mesh shape keys, exported as morph targets. Their values range from
zero to one. The browser owns blink timing and chooses a default expression for
each clip. A manual expression takes priority. Do not animate those same values
from a second controller.

`Socket_Head`, `Socket_Face`, and `Socket_Back`
follow the skeleton. Attach new objects to those nodes. The model uses meters.
In Blender it is Z-up and faces -Y. The GLB is Y-up and faces +Z.

The head, ears, sprout, and feet use rigid bone weights. This is a stylized toy
rig. The six authored clips stay within its range.
It has no speech mouth shapes or lip sync.

## Pagis use

See [the sprite integration guide](../README.md) for saved appearance, motion
rules, and the steps to add another sprite. `ui/src/avatars/SpriteAvatar.tsx`
is the shared component. Each character owns its skeleton and materials. It
shares mesh geometry and textures. A state change fades between clips over
0.25 seconds and can interrupt Celebrate or Error.

## Edit and export

Open `pixie.blend` and select the `Pixie_Studio` scene. `Pixie_Character` contains
the asset. `Pixie_Lighting` contains the camera and lights. The original startup
scene is kept in the file.

The six clips are stashed as named tracks on `Pixie_Rig`. They are muted so
they do not add together. Select an action to edit or play it. The file opens
with Idle active. The glasses and scarf are separate objects. Show them in the
viewport when you edit those parts.

The `asset_folder` property on the scene is `//`, the folder of the `.blend`
file, so the asset pack can move. The image and render paths are relative in the
same way. Run `source/export_model.py` in the Blender Python console
or Text Editor to write all four exports. The exporter restores the selection,
colors, and viewport visibility after it finishes.

To build from source, start a new Blender file, set `PIXIE_DIR` to the absolute
asset folder path, and run `source/build_model.py`. Then run `source/animate.py`
and `source/export_model.py`. Run `source/portrait.py` with `PIXIE_PRESET` set to
`mint`, `lavender`, or `peach` to render a portrait. Wait for that render before
starting another. Set `PIXIE_STUDIO=True` for a portrait with the studio ground.
Set it to `False` for the transparent portrait. Run `source/render_portraits.py`
to render all four portraits in sequence and restore the Mint source scene.
The saved blend file is the editable source of truth.

Materials use base color, roughness, and metallic values that export directly.
The blush and inner ears use vertex color. The leaves, body, scarf, and leather
use the packed surface normal image in `textures/surface-normal.png`. The GLB
includes that image, UV coordinates, and tangent data. It contains no procedural
shader that needs baking.

## Validation

Run the nine stored Python export checks from the repository root:

```sh
python3 -m unittest discover -s assets/avatars/pixie/tests -v
```

Run the real Three.js browser-loader checks with:

```sh
npm --prefix ui test -- src/avatars
```

The loader tests use the real GLBs. They decode embedded PNGs with `fast-png`
because jsdom has no browser image decoder. The live browser uses its normal
image decoder. The Khronos report records zero errors and warnings for all
four exports. Information notices concern unused UV and tangent data on
untextured parts of shared meshes.
