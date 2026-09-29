"""Set a preset and start a transparent portrait render.

Set PIXIE_PRESET to mint, lavender, or peach before execution.
"""

import bpy
import json
from pathlib import Path

scene=bpy.context.scene
studio_portrait=globals().get('PIXIE_STUDIO',False)
bpy.data.objects['Portrait ground'].hide_render=not studio_portrait
bpy.data.objects['Portrait ground'].is_shadow_catcher=False
scene.render.film_transparent=not studio_portrait
root=Path(bpy.path.abspath(scene['asset_folder']))
preset=json.loads((root.parent/'catalog.json').read_text())['pixie']['presets'][PIXIE_PRESET]

def linear(h):
    h=h.lstrip('#')
    rgb=[int(h[i:i+2],16)/255 for i in (0,2,4)]
    return tuple(v/12.92 if v<=.04045 else ((v+.055)/1.055)**2.4 for v in rgb)

for name,color in preset['materials'].items():
    mat=bpy.data.materials[name]
    mat.diffuse_color=(*linear(color),1)
    mat.node_tree.nodes['Principled BSDF'].inputs['Base Color'].default_value=mat.diffuse_color
for obj in bpy.data.collections['Pixie_Character'].objects:
    if 'accessory' in obj:
        hidden=not preset['accessories'][obj['accessory']]
        obj.hide_render=hidden
        obj.hide_set(hidden)
scene.frame_set(1)
suffix='-studio' if studio_portrait else ''
scene.render.filepath=f'//portraits/{PIXIE_PRESET}{suffix}.png'
bpy.ops.render.render('INVOKE_DEFAULT',write_still=True)
result={'preset':PIXIE_PRESET,'file':scene.render.filepath}
