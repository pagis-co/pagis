"""Export the full customization model and three standalone colorways."""

import bpy
import json
from pathlib import Path


def export_character(preset_name=None):
    scene = bpy.context.scene
    root = Path(bpy.path.abspath(scene['asset_folder']))
    collection = bpy.data.collections['Pixie_Character']
    presets = json.loads((root.parent/'catalog.json').read_text())['pixie']['presets']
    preset = presets[preset_name or 'mint']
    filename = 'pixie.glb' if preset_name is None else f'pixie-{preset_name}.glb'
    path = root/'exports'/filename
    if bpy.context.object and bpy.context.object.mode != 'OBJECT':
        bpy.ops.object.mode_set(mode='OBJECT')
    selected = list(bpy.context.selected_objects)
    active = bpy.context.view_layer.objects.active
    hidden = {obj: obj.hide_get() for obj in collection.objects}
    colors = {}
    try:
        for name, color in preset['materials'].items():
            mat = bpy.data.materials[name]
            colors[mat] = (tuple(mat.diffuse_color), tuple(mat.node_tree.nodes['Principled BSDF'].inputs['Base Color'].default_value))
            rgb = [int(color[i:i+2], 16)/255 for i in (1, 3, 5)]
            linear = tuple(v/12.92 if v <= .04045 else ((v+.055)/1.055)**2.4 for v in rgb)
            mat.diffuse_color = (*linear, 1)
            mat.node_tree.nodes['Principled BSDF'].inputs['Base Color'].default_value = mat.diffuse_color
        bpy.ops.object.select_all(action='DESELECT')
        for obj in collection.objects:
            obj.hide_set(False)
            include = preset_name is None or 'accessory' not in obj or preset['accessories'][obj['accessory']]
            obj.select_set(include)
        bpy.context.view_layer.objects.active = bpy.data.objects['Pixie_Rig']
        bpy.ops.export_scene.gltf(
            filepath=str(path), export_format='GLB', use_selection=True,
            use_active_scene=True, export_extras=True, export_yup=True,
            export_animations=True, export_animation_mode='ACTIONS',
            export_merge_animation='ACTION', export_anim_slide_to_zero=True,
            export_skins=True, export_armature_object_remove=True,
            export_texcoords=True, export_tangents=True, export_morph=True, export_morph_animation=True,
            export_apply=False, export_cameras=False, export_lights=False,
        )
    finally:
        bpy.ops.object.select_all(action='DESELECT')
        for obj, value in hidden.items():
            obj.hide_set(value)
        for mat, (viewport, shader) in colors.items():
            mat.diffuse_color = viewport
            mat.node_tree.nodes['Principled BSDF'].inputs['Base Color'].default_value = shader
        for obj in selected:
            obj.select_set(True)
        bpy.context.view_layer.objects.active = active
    return {'glb': str(path), 'bytes': path.stat().st_size}


result = {'exports': [export_character(name) for name in (None, 'mint', 'lavender', 'peach')]}
