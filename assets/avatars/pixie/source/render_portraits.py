"""Render all portraits, then restore and save the Mint source scene."""

import bpy
from pathlib import Path


def render_portraits():
    if bpy.app.is_job_running('RENDER'):
        raise RuntimeError('Wait for the current render before starting the portrait batch.')
    root=Path(bpy.path.abspath(bpy.context.scene['asset_folder']))
    source=(root/'source/portrait.py').read_text()
    queue=[('mint',False),('lavender',False),('peach',False),('mint',True)]
    bpy.context.scene['portrait_queue_done']=False

    def render_next():
        if bpy.app.is_job_running('RENDER'): return 1.0
        if queue:
            preset,studio=queue.pop(0)
            bpy.context.scene['portrait_queue_remaining']=len(queue)
            exec(compile(source,'portrait.py','exec'),{'PIXIE_PRESET':preset,'PIXIE_STUDIO':studio})
            return 1.0
        exec(compile(source.split('scene.render.filepath=')[0],'restore_mint.py','exec'),{'PIXIE_PRESET':'mint','PIXIE_STUDIO':False})
        bpy.context.scene['portrait_queue_done']=True
        bpy.ops.wm.save_as_mainfile(filepath=str(root/'pixie.blend'))
        bpy.app.driver_namespace.pop('pixie_render_step',None)
        return None

    bpy.app.driver_namespace['pixie_render_step']=render_next
    bpy.app.timers.register(render_next,first_interval=.1)


render_portraits()
result={'state':'rendering','portraits':['mint','lavender','peach','mint-studio']}
