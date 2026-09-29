"""Author six head, ear, and spin clips. Facial controls remain independent."""

import bpy
import math

scene=bpy.context.scene
rig=bpy.data.objects['Pixie_Rig']
rig.animation_data_create()
for track in list(rig.animation_data.nla_tracks):
    rig.animation_data.nla_tracks.remove(track)
rig.animation_data.action=None
clips={'Idle':120,'Working':90,'Waiting':150,'NeedsInput':96,'Celebrate':72,'Error':90}

for name,duration in clips.items():
    old=bpy.data.actions.get(name)
    if old:
        bpy.data.actions.remove(old)
    action=bpy.data.actions.new(name)
    action.use_fake_user=True
    rig.animation_data.action=action
    frames=list(range(1,duration+2,3))
    if frames[-1]!=duration+1: frames.append(duration+1)
    for frame in frames:
        t=(frame-1)/duration
        wave=math.sin(t*math.tau)
        for bone in rig.pose.bones:
            bone.rotation_mode='XYZ'
            bone.location=(0,0,0)
            bone.rotation_euler=(0,0,0)
            bone.scale=(1,1,1)
        head=rig.pose.bones['Head']
        root=rig.pose.bones['Root']
        # Hands, body, feet, and hair have no independent movement.
        head.rotation_euler.z=.14+.025*wave
        for side,sign in [('L',-1),('R',1)]:
            rig.pose.bones['Ear.'+side].rotation_euler.x=sign*.025*wave
        if name=='Working':
            head.rotation_euler.x=.09+.045*math.sin(t*math.tau*2)
            for side,sign in [('L',-1),('R',1)]:
                rig.pose.bones['Ear.'+side].rotation_euler.x=sign*.07*math.sin(t*math.tau*2+sign*.7)
        elif name=='Waiting':
            head.rotation_euler.z=.14+.10*wave
            head.rotation_euler.y=.05*math.sin(t*math.tau)
            for side in ('L','R'): rig.pose.bones['Ear.'+side].rotation_euler.x=.045*wave
        elif name=='NeedsInput':
            head.rotation_euler.y=.09
            head.rotation_euler.z=.24+.03*wave
            rig.pose.bones['Ear.R'].rotation_euler.x=.12+.08*wave
            rig.pose.bones['Ear.L'].rotation_euler.x=-.04
        elif name=='Celebrate':
            envelope=math.sin(math.pi*t)**2
            root.rotation_euler.y=math.tau*(3*t*t-2*t*t*t)
            head.rotation_euler.x=-.13*envelope
            for side,sign in [('L',-1),('R',1)]:
                rig.pose.bones['Ear.'+side].rotation_euler.x=sign*.23*math.sin(t*math.tau*3)*envelope
        elif name=='Error':
            envelope=math.sin(math.pi*t)**2
            head.rotation_euler.z=.14+.18*math.sin(t*math.tau*2)*envelope
            head.rotation_euler.x=.15*envelope
            for side,sign in [('L',-1),('R',1)]:
                rig.pose.bones['Ear.'+side].rotation_euler.z=sign*.18*envelope
        for bone in rig.pose.bones:
            if bone.name not in {'Root','Head','Ear.L','Ear.R'}: continue
            bone.keyframe_insert('location',frame=frame,group=bone.name)
            bone.keyframe_insert('rotation_euler',frame=frame,group=bone.name)
            bone.keyframe_insert('scale',frame=frame,group=bone.name)
    slot=rig.animation_data.action_slot
    track=rig.animation_data.nla_tracks.new()
    track.name=name
    strip=track.strips.new(name,1,action)
    strip.action_slot=slot
    track.mute=True
    rig.animation_data.action=None

rig.animation_data.action=bpy.data.actions['Idle']
rig.animation_data.action_slot=bpy.data.actions['Idle'].slots[0]
scene.frame_start=1
scene.frame_end=121
scene.frame_set(1)
for obj in bpy.data.collections['Pixie_Character'].objects:
    if obj.type=='MESH' and obj.data.shape_keys:
        for key in obj.data.shape_keys.key_blocks: key.value=0
bpy.ops.wm.save_as_mainfile(filepath=bpy.data.filepath)
result={'clips':clips,'bones':len(rig.pose.bones)}
