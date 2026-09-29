"""Blender geometry, face controls, and export for Brownie and Pebble."""
import bpy
import bmesh
import math
from mathutils import Vector
from pathlib import Path


def linear(value):
    value = value.lstrip('#')
    rgb = [int(value[i:i+2], 16) / 255 for i in (0, 2, 4)]
    return tuple(v / 12.92 if v <= .04045 else ((v + .055) / 1.055) ** 2.4 for v in rgb)


class Character:
    def __init__(self, family, root):
        self.family, self.root = family, Path(root)
        self.scene = bpy.data.scenes.new(family.title() + '_Studio')
        bpy.context.window.scene = self.scene
        self.collection = bpy.data.collections.new(family.title() + '_Character')
        self.scene.collection.children.link(self.collection)
        self.materials, self.parts, self.eyes, self.mouth = {}, [], [], []
        self.optional = {}

    def material(self, name, color, roughness=.6, grain=0):
        mat = bpy.data.materials.new(name)
        mat.use_nodes = True
        mat.diffuse_color = (*linear(color), 1)
        shader = mat.node_tree.nodes['Principled BSDF']
        shader.inputs['Base Color'].default_value = mat.diffuse_color
        shader.inputs['Roughness'].default_value = roughness
        self.materials[name] = mat
        if grain:
            # A packed normal texture preserves fine grain in the browser.
            import numpy as np
            rng = np.random.default_rng(732)
            h = rng.random((256, 256))
            h = (h + np.roll(h, 1, 0) + np.roll(h, 1, 1)) / 3
            x = (np.roll(h, -1, 1) - np.roll(h, 1, 1)) * grain
            y = (np.roll(h, -1, 0) - np.roll(h, 1, 0)) * grain
            n = np.sqrt(x*x+y*y+1)
            pixels = np.stack((x/n*.5+.5, y/n*.5+.5, 1/n*.5+.5, np.ones_like(x)), -1)
            image = bpy.data.images.new(name+' grain', width=256, height=256, alpha=True)
            image.colorspace_settings.name = 'Non-Color'
            image.pixels.foreach_set(pixels.astype(np.float32).ravel())
            image.pack()
            tex = mat.node_tree.nodes.new('ShaderNodeTexImage'); tex.image = image
            normal = mat.node_tree.nodes.new('ShaderNodeNormalMap')
            mat.node_tree.links.new(tex.outputs['Color'], normal.inputs['Color'])
            mat.node_tree.links.new(normal.outputs['Normal'], shader.inputs['Normal'])
        return mat

    def select(self, objects):
        bpy.ops.object.select_all(action='DESELECT')
        for obj in objects: obj.select_set(True)
        bpy.context.view_layer.objects.active = objects[0]

    def place(self, obj, name, material, bone='Head', group=None):
        obj.name = name
        for collection in list(obj.users_collection): collection.objects.unlink(obj)
        self.collection.objects.link(obj)
        obj.data.materials.append(self.materials[material])
        for polygon in obj.data.polygons: polygon.use_smooth = True
        weight = obj.vertex_groups.new(name=bone)
        weight.add(list(range(len(obj.data.vertices))), 1, 'REPLACE')
        (self.parts if group is None else group).append(obj)
        return obj

    def sphere(self, name, position, scale, material, bone='Head', group=None, segments=40, rings=28):
        bpy.ops.mesh.primitive_uv_sphere_add(segments=segments, ring_count=rings, location=position)
        obj = bpy.context.object; obj.scale = scale
        bpy.ops.object.transform_apply(location=True, rotation=True, scale=True)
        return self.place(obj, name, material, bone, group)

    def mesh(self, name, vertices, faces, material, bone='Head', group=None):
        data = bpy.data.meshes.new(name); data.from_pydata(vertices, [], faces); data.update()
        obj = bpy.data.objects.new(name, data)
        bm = bmesh.new(); bm.from_mesh(data)
        bmesh.ops.recalc_face_normals(bm, faces=list(bm.faces)); bm.to_mesh(data); bm.free()
        return self.place(obj, name, material, bone, group)

    def tube(self, name, points, radius, material, bone='Head', group=None, cyclic=False):
        curve = bpy.data.curves.new(name, 'CURVE'); curve.dimensions = '3D'
        curve.bevel_depth = radius; curve.bevel_resolution = 3
        line = curve.splines.new('POLY'); line.points.add(len(points)-1)
        for point, value in zip(line.points, points): point.co = (*value, 1)
        line.use_cyclic_u = cyclic
        obj = bpy.data.objects.new(name, curve); self.collection.objects.link(obj)
        self.select([obj]); bpy.ops.object.convert(target='MESH')
        return self.place(bpy.context.object, name, material, bone, group)

    def leaf(self, name, points, width, material, bone='Head', depth=.025):
        a, b, c, d = map(Vector, points)
        def curve(t): return (1-t)**3*a+3*(1-t)**2*t*b+3*(1-t)*t*t*c+t**3*d
        vertices, faces, vein = [], [], []
        for i in range(33):
            t = i/32; p = curve(t)
            tangent = (curve(min(1,t+.001))-curve(max(0,t-.001))).normalized()
            across = tangent.cross(Vector((0,-1,0))).normalized()
            bulge = max(.0001, math.sin(math.pi*t))**.75
            vein.append(tuple(p+Vector((0,-depth*bulge-.003,0))))
            for j in range(16):
                angle = math.tau*j/16
                vertices.append(tuple(p+across*math.cos(angle)*width*bulge+Vector((0,-math.sin(angle)*depth*bulge,0))))
        for i in range(32):
            for j in range(16):
                k=i*16+j; nxt=i*16+(j+1)%16
                faces.append((k,nxt,nxt+16,k+16))
        faces += [tuple(reversed(range(16))), tuple(512+j for j in range(16))]
        obj = self.mesh(name,vertices,faces,material,bone)
        if material=='Leaves':
            self.tube(name+' vein',vein[2:-2],.003,'Vein',bone)
            for start in (.28,.46,.64):
                for side in (-1,1):
                    points=[]
                    for step in range(9):
                        u=step/8*.76; t=start+step/8*.12; p=curve(t)
                        tangent=(curve(t+.001)-curve(t-.001)).normalized()
                        across=tangent.cross(Vector((0,-1,0))).normalized()
                        bulge=math.sin(math.pi*t)**.75
                        points.append(tuple(p+across*(side*u*width*bulge)+Vector((0,-depth*bulge*math.sqrt(1-u*u)-.001,0))))
                    self.tube(name+' branch vein',points,.0015,'Vein',bone)
        return obj

    def face(self, eye_x, eye_z, eye_y, mouth_z, mouth_y, eye_size=.14, open_mouth=True):
        self.eye_z, self.mouth_z = eye_z, mouth_z
        for side in (-1,1):
            x = side*eye_x
            self.sphere('Eye rim',(x,eye_y+.014,eye_z),(eye_size*1.08,.04,eye_size*1.1),'EyeRim',group=self.eyes)
            self.sphere('Eye',(x,eye_y,eye_z),(eye_size,.055,eye_size*1.06),'Eyes',group=self.eyes)
            self.sphere('Eye light',(x-.035,eye_y-.052,eye_z+.052),(.035,.009,.038),'White',group=self.eyes,segments=24,rings=16)
            self.sphere('Eye glint',(x+.040,eye_y-.051,eye_z-.052),(.009,.006,.01),'White',group=self.eyes,segments=16,rings=12)
        vertices=[(0,mouth_y,mouth_z-.045)]
        for i in range(64):
            angle=math.tau*i/64; u=math.cos(angle); s=math.sin(angle)
            z=mouth_z-.02*(1-u*u) if s>=0 else mouth_z-.10*(-s)
            vertices.append((.12*u,mouth_y,z))
        if open_mouth:
            self.mesh('Smile',vertices,[(0,i+1,(i+1)%64+1) for i in range(64)],'Mouth',group=self.mouth)
            self.sphere('Tongue',(0,mouth_y-.008,mouth_z-.075),(.064,.007,.027),'Pink',group=self.mouth)
        else:
            self.tube('Smile',[(.14*u,mouth_y,mouth_z+.065*u*u) for u in [i/20 for i in range(-20,21)]],.011,'Mouth',group=self.mouth)

    def blush(self, obj, base, centers, size):
        mat = self.materials[base]
        node=mat.node_tree.nodes.new('ShaderNodeVertexColor'); node.layer_name='Color'
        mix=mat.node_tree.nodes.new('ShaderNodeMixRGB'); mix.blend_type='MULTIPLY'; mix.inputs[0].default_value=1
        mix.inputs[1].default_value=mat.diffuse_color
        mat.node_tree.links.new(node.outputs['Color'],mix.inputs[2])
        mat.node_tree.links.new(mix.outputs['Color'],mat.node_tree.nodes['Principled BSDF'].inputs['Base Color'])
        # Vertex colors hold a tint so the user can change the base color.
        attr=obj.data.color_attributes.new(name='Color',type='FLOAT_COLOR',domain='POINT')
        for v, pixel in zip(obj.data.vertices,attr.data):
            x,y,z=v.co
            d=min(((x-cx)/size[0])**2+((z-cz)/size[1])**2 for cx,cz in centers)
            amount=math.exp(-d)*.90 if y<-.1 else 0
            pixel.color=(1,1-amount*.65,1-amount*.25,1)

    def join(self, parts, name):
        for part in parts:
            if not part.data.color_attributes.get('Color'):
                attr=part.data.color_attributes.new(name='Color',type='FLOAT_COLOR',domain='POINT')
                for pixel in attr.data: pixel.color=(1,1,1,1)
        self.select(parts); bpy.ops.object.join(); obj=bpy.context.object; obj.name=name
        bm=bmesh.new(); bm.from_mesh(obj.data); bmesh.ops.triangulate(bm,faces=list(bm.faces)); bm.to_mesh(obj.data); bm.free()
        return obj

    def finish(self, head_pivot, height, camera_target):
        base=self.join(self.parts,self.family.title())
        eyes=self.join(self.eyes,'FaceEyes'); mouth=self.join(self.mouth,'FaceMouth')
        for obj in (eyes,mouth): obj.shape_key_add(name='Basis')
        blink=eyes.shape_key_add(name='Blink', from_mix=False); blink.value=0
        for p in blink.data:
            p.co.z=self.eye_z+(p.co.z-self.eye_z)*.07
            p.co.y+=.026
        for name,sx,sz,dz in [('Smile',1.2,1.15,0),('Surprise',.55,1.25,.01),('Concern',1,.15,-.015)]:
            key=mouth.shape_key_add(name=name, from_mix=False); key.value=0
            for p in key.data:
                if name=='Surprise' and self.family=='brownie':
                    u=max(-1,min(1,p.co.x/.14)); angle=(u+1)*math.pi
                    thickness=p.co.z-self.mouth_z-.065*u*u
                    p.co.x=.065*math.cos(angle)
                    p.co.z=self.mouth_z+.015+.077*math.sin(angle)+thickness
                else:
                    p.co.x*=sx; p.co.z=self.mouth_z+(p.co.z-self.mouth_z)*sz+dz
                    if name=='Concern': p.co.z-=.22*abs(p.co.x)
        meshes=[base,eyes,mouth]
        for name,parts in self.optional.items():
            obj=self.join(parts,'Accessory_'+name.title()); obj['accessory']=name; meshes.append(obj)
        arm=bpy.data.armatures.new(self.family.title()+'_Rig')
        rig=bpy.data.objects.new(self.family.title()+'_Rig',arm); self.collection.objects.link(rig)
        self.select([rig]); bpy.ops.object.mode_set(mode='EDIT')
        for name,head,tail,parent in [('Root',(0,0,0),(0,0,.3),None),('Body',(0,0,.3),(0,0,head_pivot),'Root'),('Head',(0,0,head_pivot),(0,0,height),'Body'),('Ear.L',(-.5,0,height*.8),(-.8,0,height*.9),'Head'),('Ear.R',(.5,0,height*.8),(.8,0,height*.9),'Head')]:
            bone=arm.edit_bones.new(name); bone.head=head; bone.tail=tail
            if parent: bone.parent=arm.edit_bones[parent]
        bpy.ops.object.mode_set(mode='OBJECT')
        for obj in meshes:
            obj.parent=rig; mod=obj.modifiers.new('Character skin','ARMATURE'); mod.object=rig
        self.rig=rig
        rig['family']=self.family
        self.animate()
        self.studio(height,camera_target)
        self.scene['asset_folder']='//'
        self.scene['reference_note']='Front view follows the supplied illustration. Back and sides are interpreted.'
        bpy.context.preferences.filepaths.save_version=0
        bpy.ops.wm.save_as_mainfile(filepath=str(self.root/(self.family+'.blend')),compress=True)
        self.select(list(self.collection.objects))
        painted=[]
        for mat in self.materials.values():
            socket=mat.node_tree.nodes['Principled BSDF'].inputs['Base Color']
            if socket.links:
                link=socket.links[0]; painted.append((mat,link.from_socket,socket)); mat.node_tree.links.remove(link)
                socket.default_value=mat.diffuse_color
        bpy.ops.export_scene.gltf(filepath=str(self.root/'exports'/(self.family+'.glb')),export_format='GLB',use_selection=True,use_active_scene=True,export_extras=True,export_yup=True,export_animations=True,export_animation_mode='ACTIONS',export_merge_animation='ACTION',export_anim_slide_to_zero=True,export_skins=True,export_armature_object_remove=True,export_morph=True,export_vertex_color='ACTIVE',export_all_vertex_colors=False,export_tangents=True,export_cameras=False,export_lights=False)
        for mat,source,target in painted: mat.node_tree.links.new(source,target)
        self.scene.render.filepath=str(self.root/'portraits'/'classic.png')
        bpy.ops.render.render(write_still=True)

    def animate(self):
        rig=self.rig; rig.animation_data_create()
        for name,duration in {'Idle':120,'Working':90,'Waiting':150,'NeedsInput':96,'Celebrate':72,'Error':90}.items():
            action=bpy.data.actions.new(name); action.use_fake_user=True; rig.animation_data.action=action
            for frame in range(1,duration+2,3):
                t=(frame-1)/duration; wave=math.sin(t*math.tau); envelope=math.sin(math.pi*t)**2
                for bone in rig.pose.bones:
                    bone.rotation_mode='XYZ'; bone.rotation_euler=(0,0,0)
                head=rig.pose.bones['Head']; root=rig.pose.bones['Root']
                head.rotation_euler.y=.018*wave
                head.rotation_euler.z=.24 if self.family=='brownie' else 0
                if name=='Working': head.rotation_euler.x=.045+.04*math.sin(t*math.tau*2)
                elif name=='Waiting': head.rotation_euler.z+=(.08*wave)
                elif name=='NeedsInput': head.rotation_euler.y=.10+.03*wave
                elif name=='Celebrate':
                    root.rotation_euler.y=math.tau*(3*t*t-2*t*t*t)
                    head.rotation_euler.x=-.10*envelope
                elif name=='Error': head.rotation_euler.z+=.14*math.sin(t*math.tau*2)*envelope
                for side,sign in [('L',-1),('R',1)]: rig.pose.bones['Ear.'+side].rotation_euler.x=sign*.04*wave
                for bone in rig.pose.bones:
                    if bone.name!='Body': bone.keyframe_insert('rotation_euler',frame=frame,group=bone.name)
            slot=rig.animation_data.action_slot
            track=rig.animation_data.nla_tracks.new(); track.name=name
            strip=track.strips.new(name,1,action); strip.action_slot=slot; track.mute=True
            rig.animation_data.action=None
        rig.animation_data.action=bpy.data.actions['Idle']; rig.animation_data.action_slot=bpy.data.actions['Idle'].slots[0]
        self.scene.frame_set(1)

    def studio(self,height,target):
        scene=self.scene; scene.render.engine='CYCLES'; scene.cycles.samples=48; scene.cycles.use_denoising=True
        scene.render.resolution_x=800; scene.render.resolution_y=800; scene.render.resolution_percentage=100
        scene.render.image_settings.file_format='PNG'; scene.render.image_settings.color_mode='RGBA'; scene.render.film_transparent=True
        scene.render.fps=30; scene.view_settings.view_transform='AgX'
        world=bpy.data.worlds.new(self.family+' world'); world.use_nodes=True
        world.node_tree.nodes['Background'].inputs['Color'].default_value=(.8,.75,.68,1)
        world.node_tree.nodes['Background'].inputs['Strength'].default_value=.3; scene.world=world
        def aim(obj): obj.rotation_euler=(Vector((0,0,target))-obj.location).to_track_quat('-Z','Y').to_euler()
        data=bpy.data.cameras.new('Portrait'); camera=bpy.data.objects.new('Portrait',data); scene.collection.objects.link(camera)
        camera.location=(.15,-10,2.6); data.type='ORTHO'; data.ortho_scale=height*1.16; aim(camera); scene.camera=camera
        for name,location,power,size in [('Key',(-3,-4,6),500,4),('Fill',(4,-2,3),170,3),('Rim',(1,3,5),450,3)]:
            light=bpy.data.lights.new(name,'AREA'); light.energy=power; light.shape='DISK'; light.size=size
            obj=bpy.data.objects.new(name,light); scene.collection.objects.link(obj); obj.location=location; aim(obj)
